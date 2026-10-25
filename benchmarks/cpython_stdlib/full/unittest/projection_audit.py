"""Build intact unittest method projections with the pinned test tree at O0 and O3.

The CPython runner remains the authority for discovery. A returned projection
is a lead for the native fixture runner; this sweep alone never awards passes.
"""

import argparse
import ast
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import hashlib
from functools import cache
import inspect
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
PIN = "5607950ef232dad16d75c0cf53101d9649d89115"
TEST_TREE = "b4324777842d3e16d2f95dc24789b2a80486541a"
MARK = b"completed native method projection\n"


@cache
def parsed(path):
    raw = (HERE / "upstream" / path).read_bytes()
    tree = ast.parse(raw)
    functions = {}
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            first = min([node.lineno, *(d.lineno for d in node.decorator_list)])
            functions.setdefault((node.name, first), []).append(node)
    return raw.splitlines(keepends=True), tree, functions


def definition(function, files):
    function = inspect.unwrap(function)
    code = getattr(function, "__code__", None)
    if code is None:
        raise ValueError(f"{function.__qualname__} has no Python definition")
    base = Path(code.co_filename).parts
    if "test_unittest" not in base:
        raise ValueError(f"{function.__qualname__} is outside the pinned package")
    path = str(Path("Lib/test", *base[base.index("test_unittest") :]))
    if path not in files:
        raise ValueError(f"{path} has no pinned blob")
    lines, tree, functions = parsed(path)
    nodes = functions.get((function.__name__, code.co_firstlineno), ())
    location = f"{path}:{code.co_firstlineno}"
    if len(nodes) != 1:
        raise ValueError(
            f"{location}: runtime definition has no unique pinned byte range"
        )
    (node,) = nodes
    if isinstance(node, ast.AsyncFunctionDef) or inspect.isgeneratorfunction(function):
        raise ValueError(f"{location}: calling this method alone does not execute its body")
    if node.decorator_list:
        decorators = ", ".join(ast.unparse(d) for d in node.decorator_list)
        raise ValueError(f"{location}: preserve upstream decorators: {decorators}")
    if node.col_offset != 4:
        raise ValueError(
            f"{location}: definition requires the original enclosing scope"
        )
    return path, tree, node, lines[node.lineno - 1 : node.end_lineno]


def candidate(case, files):
    """Keep the method and its direct self helpers/instance hooks byte-for-byte.

    This candidate does not reproduce module globals, class attributes, class hooks
    or decorators. A successful check is only a reason to verify the entire fixture.
    """
    todo = [case._testMethodName, "setUp", "tearDown"]
    parts, imports = {}, {}
    while todo:
        name = todo.pop(0)
        if name in parts:
            continue
        function = getattr(type(case), name)
        if not inspect.isfunction(function):
            raise ValueError(f"{name} requires a descriptor or generated callable")
        if inspect.unwrap(function).__module__.startswith("unittest."):
            continue
        path, tree, method, lines = definition(function, files)
        parts[name] = (lines, path, method.lineno)
        used = set()
        for node in ast.walk(method):
            if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load):
                used.add(node.id)
            if (
                isinstance(node, ast.Attribute)
                and isinstance(node.value, ast.Name)
                and node.value.id == "self"
                and inspect.isfunction(getattr(type(case), node.attr, None))
                and node.attr not in parts
                and node.attr not in todo
            ):
                todo.append(node.attr)
        for stmt in tree.body:
            if not isinstance(stmt, (ast.Import, ast.ImportFrom)):
                continue
            for alias in stmt.names:
                if (alias.asname or alias.name.split(".")[0]) not in used:
                    continue
                prefix = "import "
                if isinstance(stmt, ast.ImportFrom):
                    prefix = f"from {'.' * stmt.level}{stmt.module or ''} import "
                imports[prefix + ast.unparse(alias)] = f"{path}:{stmt.lineno}"
    if case._testMethodName not in parts:
        raise ValueError("the test method does not originate in the pinned suite")

    source, locations = [], []

    def line(data, location=None):
        source.append(data)
        locations.append(location)

    line(b"import unittest\n")
    for stmt, location in sorted(imports.items()):
        line(stmt.encode() + b"\n", location)
    line(b"class Candidate(unittest.TestCase):\n")
    for lines, path, start in parts.values():
        for n, data in enumerate(lines, start):
            line(data, f"{path}:{n}")
        line(b"\n")
    for data in (
        "subject = Candidate()",
        "subject.setUp()",
        "try:",
        f"    subject.{case._testMethodName}()",
        "finally:",
        "    subject.tearDown()",
    ):
        line(data.encode() + b"\n")
    return b"".join(source), locations


def git_hash(kind, raw):
    return hashlib.sha1(kind + b" " + str(len(raw)).encode() + b"\0" + raw).digest()


def stage_test_tree(source, root, receipt):
    if receipt["truncated"] or receipt["sha"] != TEST_TREE:
        raise ValueError("incomplete or wrong pinned Lib/test tree")
    trie = {}
    for row in receipt["files"]:
        path = Path(row["path"])
        if (
            path.is_absolute()
            or ".." in path.parts
            or row["mode"] not in {"100644", "100755"}
        ):
            raise ValueError(row)
        node = trie
        for name in path.parts[:-1]:
            node = node.setdefault(name, {})
        if path.name in node:
            raise ValueError("duplicate fixture " + str(path))
        raw = (source / "Lib/test" / path).read_bytes()
        digest = git_hash(b"blob", raw)
        if digest.hex() != row["sha"]:
            raise ValueError("fixture blob mismatch: " + str(path))
        node[path.name] = (row["mode"].encode(), digest)
        dest = root / "test" / path
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(raw)
        if row["mode"] == "100755":
            dest.chmod(0o755)

    def subtree(node):
        data = bytearray()
        for key, val in sorted(
            node.items(),
            key=lambda pair: (
                pair[0] + ("/" if isinstance(pair[1], dict) else "")
            ).encode(),
        ):
            mode, sha = (b"40000", subtree(val)) if isinstance(val, dict) else val
            data.extend(mode + b" " + key.encode() + b"\0" + sha)
        return git_hash(b"tree", bytes(data))

    if subtree(trie).hex() != TEST_TREE:
        raise ValueError("fixture list does not reconstruct CPython's tree")
    return len(receipt["files"])


def process(command, work, timeout):
    p = subprocess.Popen(
        command,
        cwd=work,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        out, err = p.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, signal.SIGKILL)
        p.communicate()
        return None, b"", b""
    return p.returncode, out, err


def method_metadata(case, files):
    fn = inspect.unwrap(getattr(type(case), case._testMethodName))
    code = getattr(fn, "__code__", None)
    if code is None or "test_unittest" not in Path(code.co_filename).parts:
        return {"qualname": getattr(fn, "__qualname__", case._testMethodName)}
    parts = Path(code.co_filename).parts
    path = str(Path("Lib/test", *parts[parts.index("test_unittest") :]))
    if path not in files:
        raise ValueError(path)
    lines, _, functions = parsed(path)
    nodes = functions.get((fn.__name__, code.co_firstlineno), ())
    result = {"source": f"{path}:{code.co_firstlineno}", "qualname": fn.__qualname__}
    if len(nodes) == 1:
        result["definition_sha256"] = hashlib.sha256(
            b"".join(lines[code.co_firstlineno - 1 : nodes[0].end_lineno])
        ).hexdigest()
    return result


def try_case(args, root, source, locations, opt, reachable):
    with tempfile.TemporaryDirectory(dir=root, prefix="probe_") as probe:
        work = Path(probe)
        # The compiler resolves the entry as a module relative to the pinned root.
        (work / "__init__.py").write_bytes(b"")
        (work / "test").symlink_to(root / "test", target_is_directory=True)
        entry = work / "candidate.py"
        entry.write_bytes(source)
        binary = work / "candidate"
        command = [
            args.imre,
            "build",
            entry,
            "--module-root",
            root,
            "--no-cache",
            "--opt-level",
            str(opt),
            "--clang",
            args.clang,
            "--runtime",
            args.runtime,
            "-o",
            binary,
        ]
        if reachable:
            command.append("--reachable-only")
        status, out, err = process(command, work, args.build_timeout)
        if status is None:
            return {"stage": "build-timeout", "seconds": args.build_timeout}
        if status:
            message = (
                (err or out)
                .decode(errors="backslashreplace")
                .replace(str(work), "<case>")
                .replace(str(root), "<pinned>")
            )
            if (
                ("cannot resolve first-party module" in message and work.name in message)
                or ("resolves outside" in message and "first-party" in message)
            ):
                raise RuntimeError(f"invalid projection staging: {message}")
            lines = [
                s
                for s in message.splitlines()
                if s and not s.lstrip().startswith("Compiling ")
            ]
            first = next(
                (
                    s
                    for s in lines
                    if "Imre:" in s or "unsupported" in s or s.startswith("error[")
                ),
                lines[0] if lines else f"build exit {status}",
            )
            detail = {
                "stage": "compile-blocked",
                "exit": status,
                "diagnostic": first,
                "diagnostic_sha256": hashlib.sha256(message.encode()).hexdigest(),
            }
            if m := re.search(r"<case>/candidate\.py:(\d+):\d+:", first):
                index = int(m[1]) - 1
                if index < len(locations) and locations[index]:
                    detail["original_source"] = locations[index]
                    detail["statement"] = (
                        source.splitlines()[index]
                        .decode(errors="backslashreplace")
                        .strip()
                    )
            elif m := re.search(r"<(?:case|pinned)>/test/(.*?):(\d+):", first):
                detail["original_source"] = "Lib/test/" + m[1] + ":" + m[2]
            return detail
        code, stdout, stderr = process([binary], work, args.run_timeout)
        if code is None:
            return {"stage": "native-run-timeout", "seconds": args.run_timeout}
        # Success and identical output do not establish the originating class's complete fixtures.
        reference, refout, referr = process(
            [sys.executable, "-E", "-B", entry], work, args.run_timeout
        )

        def norm(data):
            return data.replace(bytes(work), b"<case>").replace(
                bytes(root), b"<pinned>"
            )

        stdout, stderr, refout, referr = map(norm, (stdout, stderr, refout, referr))
        return {
            "stage": "projection-returned"
            if code == 0 and stdout.endswith(MARK)
            else "projection-failed",
            "native_exit": code,
            "reference_projection_exit": reference,
            "same_projection_output": (code, stdout, stderr)
            == (reference, refout, referr),
            "native_stdout": stdout.decode(errors="backslashreplace")[:1600],
            "native_stderr": stderr.decode(errors="backslashreplace")[:1600],
            "fixture_proof": "requires original fixture validation before module coverage credit",
        }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "cpython-source", "result"):
        p.add_argument("--" + flag, type=Path, required=True)
    p.add_argument("--fixture-tree", type=Path, default=HERE / "cpython-test-tree.json")
    p.add_argument(
        "--compiler-commit",
        required=True,
        help="Imre source revision for the measured compiler",
    )
    p.add_argument("--jobs", type=int, choices=(1, 2), default=1)
    p.add_argument("--build-timeout", type=int, default=70)
    p.add_argument("--run-timeout", type=int, default=12)
    p.add_argument(
        "--smoke",
        action="append",
        default=[],
        help="developer sample; all unscheduled IDs stay in the denominator",
    )
    a = p.parse_args()
    for name in (
        "imre",
        "runtime",
        "clang",
        "cpython_source",
        "fixture_tree",
        "result",
    ):
        setattr(a, name, getattr(a, name).resolve())
    sys.path.insert(0, str(HERE.parents[1]))
    from suite_inventory import (
        cases,
        load_suite,
        verify_files,
        verify_interpreter_tests,
    )
    from suite_evidence import occurrence_keys, reference_cases

    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    if (
        sys.version_info[:3] != (3, 14, 5)
        or sys.implementation.name != "cpython"
        or manifest["commit"] != PIN
    ):
        raise ValueError("requires the pinned CPython 3.14.5 oracle")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    allcases = list(cases(load_suite(manifest["module"])))
    ids = list(occurrence_keys(c.id() for c in allcases))
    refs = reference_cases(reference)
    if reference["cpython_commit"] != PIN or set(refs) != set(ids):
        raise ValueError("the discovered occurrences differ from the reference")
    if unknown := set(a.smoke).difference(name for name, _ in ids):
        p.error("undiscovered --smoke IDs: " + ", ".join(sorted(unknown)))
    if not re.fullmatch("[0-9a-f]{40}", a.compiler_commit):
        p.error("--compiler-commit requires the full source revision")
    plans, plan_for = {}, {}
    records = []
    for case, (name, number) in zip(allcases, ids, strict=True):
        row = {
            "id": name,
            "occurrence": number,
            "fixture_variant": type(case).__qualname__,
            "reference": refs[name, number],
            "origin": method_metadata(case, manifest["files"]),
            "modes": {},
        }
        records.append(row)
        if isinstance(row["reference"], dict) and "skipped" in row["reference"]:
            row["unattempted"] = {"stage": "reference-skip", "reason": row["reference"]["skipped"]}
            continue
        if a.smoke and name not in a.smoke:
            row["unattempted"] = {"stage": "not-scheduled", "reason": "developer sample"}
            continue
        try:
            src, loc = candidate(case, manifest["files"])
        except (OSError, ValueError, TypeError) as e:
            row["unattempted"] = {"stage": "fixture-not-projected", "reason": str(e)}
            continue
        src += b'print("completed native method projection")\n'
        digest = hashlib.sha256(src).hexdigest()
        row["projection_sha256"] = digest
        # Share a diagnostic only when identical bytes map to the same upstream lines.
        key = (digest, tuple(loc))
        plan_for[name, number] = key
        plans.setdefault(key, src)
    started = time.monotonic()
    mode_counts = {}
    receipt = json.loads(a.fixture_tree.read_bytes())
    with a.imre.open("rb") as compiler, a.runtime.open("rb") as runtime:
        compiler_sha = hashlib.file_digest(compiler, "sha256").hexdigest()
        runtime_sha = hashlib.file_digest(runtime, "sha256").hexdigest()
    with tempfile.TemporaryDirectory(prefix="imre-unittest-sweep-") as tmp:
        root = Path(tmp)
        count = stage_test_tree(a.cpython_source, root, receipt)
        print(
            f"{len(records)} discovered, {len(plans)} build plans, {count} original fixture blobs verified",
            flush=True,
        )
        for level in (0, 3):
            for reachable in (False, True):
                mode = f"{'reachable' if reachable else 'ordinary'}-O{level}"

                def execute(item):
                    key, src = item
                    return key, try_case(a, root, src, key[1], level, reachable)

                observed = {}
                with ThreadPoolExecutor(max_workers=a.jobs) as pool:
                    for index, (key, data) in enumerate(
                        pool.map(execute, plans.items()), 1
                    ):
                        observed[key] = data
                        if index % 125 == 0:
                            print(mode, index, len(plans), flush=True)
                for row in records:
                    row["modes"][mode] = (
                        observed[plan_for[row["id"], row["occurrence"]]]
                        if "projection_sha256" in row
                        else row["unattempted"]
                    )
                mode_counts[mode] = dict(
                    sorted(Counter(r["modes"][mode]["stage"] for r in records).items())
                )
                print(mode, mode_counts[mode], flush=True)
                payload = {
                    "cpython_commit": PIN,
                    "compiler_commit": a.compiler_commit,
                    "compiler_sha256": compiler_sha,
                    "runtime_sha256": runtime_sha,
                    "lib_test_tree": TEST_TREE,
                    "verified_files": count,
                    "full_denominator": len(records),
                    "smoke_ids": a.smoke,
                    "counts": mode_counts,
                    "records": records,
                }
                a.result.write_text(json.dumps(payload, indent=1) + "\n")
    print("elapsed seconds", round(time.monotonic() - started), flush=True)


if __name__ == "__main__":
    main()
