"""Try every complete upstream fnmatch body and join its evidence to the native operation run."""

import argparse
import ast
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, occurrence_keys, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import (
    NativeCompileError,
    Toolchain,
    method_bytes,
)

SOURCE = HERE / "upstream/Lib/test/test_fnmatch.py"
LINES = SOURCE.read_bytes().splitlines(keepends=True)
TOP = ast.parse(b"".join(LINES)).body


def source(node):
    return b"".join(LINES[node.lineno - 1 : node.end_lineno])


def test_entry(owner, method):
    """Keep the original class, dependencies, method and unittest.main entry point."""
    (cls,) = [n for n in TOP if isinstance(n, ast.ClassDef) and n.name == owner]
    methods = {n.name: n for n in cls.body if isinstance(n, ast.FunctionDef)}
    nodes = [methods[method]]
    helpers = []
    if any(
        isinstance(n, ast.Attribute) and n.attr == "check_match"
        for n in ast.walk(nodes[0])
    ):
        nodes.append(methods["check_match"])
        helpers.append(source(methods["check_match"]))
    names = {"unittest"} | {
        n.id for node in nodes for n in ast.walk(node) if isinstance(n, ast.Name)
    }
    assignments = [
        n
        for n in TOP
        if isinstance(n, ast.Assign)
        and any(isinstance(a, ast.Name) and a.id in names for a in n.targets)
    ]
    names.update(
        n.id for node in assignments for n in ast.walk(node) if isinstance(n, ast.Name)
    )
    imports = [
        n
        for n in TOP
        if isinstance(n, (ast.Import, ast.ImportFrom))
        and any((a.asname or a.name) in names for a in n.names)
    ]
    return (
        b"".join(source(n) for n in imports + assignments)
        + b"\n" + LINES[cls.lineno - 1]
        + b"".join(helpers)
        + method_bytes(SOURCE, owner, method)
        + b"\n" + source(TOP[-1])
    )


def check_result(command, owner, method, *, wrong=False):
    run = subprocess.run([*command, "-v"], text=True, capture_output=True, timeout=60)
    status = "FAIL" if wrong else "ok"
    rows = [s for s in run.stderr.splitlines() if " ... " in s]
    summary = "FAILED (failures=1)" if wrong else "OK"
    if (run.returncode != int(wrong) or run.stdout
            or rows != [f"{method} (__main__.{owner}.{method}) ... {status}"]
            or "Ran 1 test in " not in run.stderr
            or run.stderr.splitlines()[-1:] != [summary]):
        raise AssertionError((command, run.returncode, run.stdout, run.stderr))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "operations", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--compiler-commit", required=True)
    parser.add_argument(
        "--cpython-source",
        type=Path,
        help="pinned CPython source tree exposing Lib/test",
    )
    args = parser.parse_args()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    operations = json.loads(args.operations.read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    if (
        sys.version_info[:3] != (3, 14, 5)
        or sys.implementation.name != "cpython"
        or operations["cpython_commit"] != manifest["commit"]
    ):
        parser.error(
            "use the pinned CPython 3.14.5 test fixtures and operation evidence"
        )
    discovered = [
        (c.id(), type(c).__name__) for c in cases(load_suite(manifest["module"]))
    ]
    keys = list(occurrence_keys(n for n, _ in discovered))
    reports = []
    baseline = {r["opt_level"]: {row["id"] for row in r["records"]
                                if row["classification"] == "fully-native"}
                for r in json.loads((HERE / "evidence-linux.json").read_bytes())}
    for level in (0, 3):
        calls = operations["levels"][str(level)]
        if set(calls) != reference["reference"].keys() or any(i != 1 for _, i in keys):
            raise ValueError("operation evidence does not cover all test occurrences")
        tool = Toolchain(args.imre, args.runtime, args.clang, level)
        obs = {}
        with tempfile.TemporaryDirectory() as tmp:

            def compile_method(item):
                name, owner = item
                fn = name.rsplit(".", 1)[1]
                path = Path(tmp) / name
                path.mkdir()
                proof = {
                    "opt_level": level,
                    "compiler_commit": args.compiler_commit,
                    "method_sha256": hashlib.sha256(
                        method_bytes(SOURCE, owner, fn)
                    ).hexdigest(),
                    "native_operation_calls": calls[name],
                }
                entry = test_entry(owner, fn)
                try:
                    binary = tool.build(entry, path)
                except NativeCompileError as exc:
                    reason = str(exc).replace(str(path) + "/", "")
                    proof["complete_method_diagnostic"] = reason.strip()
                    return name, False, proof
                check_result([sys.executable, "-I", "-B", path / "case.py"], owner, fn)
                check_result([binary], owner, fn)
                if fn == "test_mix_bytes_str":
                    # Make the first call accept its pattern: the original assertRaises must fail.
                    wrong = entry.replace(b", b'*')", b", '*')", 1)
                    assert wrong != entry
                    check_result([tool.build(wrong, path)], owner, fn, wrong=True)
                elif owner in ("FilterTestCase", "FilterFalseTestCase"):
                    # Swap the public operations while leaving the original assertions intact.
                    wrong = entry.replace(b"translate, filter, filterfalse\n",
                                          b"translate, filterfalse as filter, filter as filterfalse\n", 1)
                    assert wrong != entry
                    check_result([tool.build(wrong, path)], owner, fn, wrong=True)
                return name, True, proof

            with ThreadPoolExecutor(max_workers=2) as pool:
                built = list(pool.map(compile_method, discovered))
            for name, ran, proof in built:
                if ran:
                    loc = roles(
                        body="imre",
                        assertions="imre",
                        operation="imre",
                        fixture="imre",
                    )
                    proof["fixture"] = "Original class and dependencies with native unittest.TestCase/main."
                    proof["operation"] = (
                        "The original method invokes native fnmatch and assertions in an Imre child."
                    )
                else:
                    loc = roles(
                        body="cpython",
                        assertions="cpython",
                        operation="imre",
                        fixture="cpython",
                    )
                    proof["operation"] = (
                        "CPython runs the original case and receives each requested native fnmatch result."
                    )
                    proof["fixture"] = (
                        "Original CPython TestCase, helpers, platform flags and runner."
                    )
                obs[name, 1] = {"outcome": "pass", "roles": loc, "evidence": proof}
        report = build_report(reference, level, discovered, obs)
        ran = {row["id"] for row in report["records"] if row["classification"] == "fully-native"}
        if lost := baseline[level] - ran:
            raise AssertionError(f"O{level} lost native fnmatch methods: {sorted(lost)}")
        print(f"O{level}: {report['counts']}", flush=True)
        reports.append(report)
    write_reports(args.result, reports, reference)


if __name__ == "__main__":
    main()
