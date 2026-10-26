"""Replay the twenty original default-fixture callers that import test.support."""

import argparse
import ast
from collections import Counter
import gzip
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent), str(HERE.parents[1])]
from default_fixture_audit import file_sha, try_default
from stage_tests import git_blob
from suite_evidence import classification, validate_row
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, method_bytes


def original_callers():
    # Freeze the cohort using its original recorded cursor, independently of
    # the diagnostic or outcome produced by the compiler under test.
    with gzip.open(HERE / "default-fixture-linux.json.gz") as file:
        baseline = json.load(file)["modes"]["ordinary"][0]
    return {row["id"]: row["evidence"]["method_sha256"] for row in baseline["records"]
            if row.get("evidence", {}).get("upstream_diagnostic") == "Lib/test/support/__init__.py:133:29"}


def fixture_entries(root):
    """Keep the logging support projection distinct from importing the whole test module."""
    path = root / "test/test_logging.py"
    lines = path.read_bytes().splitlines(keepends=True)
    module = ast.parse(path.read_bytes())
    cls, = (s for s in module.body if isinstance(s, ast.ClassDef) and s.name == "BasicConfigTest")
    module_setup, = (s for s in module.body if isinstance(s, ast.FunctionDef) and s.name == "setUpModule")
    imports = {b"import logging", b"import sys", b"from test import support", b"import unittest"}
    entry = b"".join(lines[s.lineno - 1] for s in module.body if isinstance(s, (ast.Import, ast.ImportFrom))
                     and lines[s.lineno - 1].rstrip() in imports)
    entry += b"\n" + lines[cls.lineno - 1]
    for name in ("setUp", "tearDown", "cleanup", "test_strformatstyle"):
        entry += method_bytes(path, cls.name, name) + b"\n"
    entry += b"".join(lines[module_setup.lineno - 1:module_setup.end_lineno])
    entry += b"\nif __name__ == '__main__':\n    unittest.main()\n"
    support = root / "test/support/__init__.py"
    init = method_bytes(support, "_MemoryWatchdog", "__init__")
    return {
        "support-import": b"import test.support\n",
        "logging-original-hooks-and-body": entry,
        "logging-whole-module": (
            b"from test.test_logging import BasicConfigTest\nimport unittest\n"
            b"result = unittest.TestResult()\n"
            b"unittest.TestSuite([BasicConfigTest('test_strformatstyle')]).run(result)\n"
            b"print(result.testsRun)\nprint(result.wasSuccessful())\n"
        ),
        "watchdog-original-initializer": (
            b"import os\nclass _MemoryWatchdog:\n" + init + b"\nwatchdog = _MemoryWatchdog()\n"
            b"print(watchdog.procfile == '/proc/{pid}/statm'.format(pid=os.getpid()))\nprint(watchdog.started)\n"
        ),
    }


def fixture_probe(tool, root, source):
    with tempfile.TemporaryDirectory(dir=root) as directory:
        work = Path(directory)
        (work / "__init__.py").touch()
        (work / "test").symlink_to(root / "test", target_is_directory=True)
        try:
            binary = tool.build(source, work)
        except NativeCompileError as exc:
            return {"stage": "compile", "diagnostic": str(exc).replace(str(work), "<case>")
                    .replace(str(root), "<root>").splitlines()[-1]}
        proc = subprocess.run([binary, "-v"], cwd=work, text=True, capture_output=True, timeout=60)
        return {"stage": "execution", "stdout": proc.stdout, "stderr": proc.stderr, "status": proc.returncode}


def reference_probe(source):
    with tempfile.TemporaryDirectory() as directory:
        entry = Path(directory) / "case.py"
        entry.write_bytes(source)
        proc = subprocess.run([sys.executable, "-I", "-B", entry, "-v"], cwd=directory,
                              text=True, capture_output=True, timeout=60, check=True)
    stderr = re.sub(r"(?m)^(Ran 1 test in )\d+\.\d+s$", r"\1<elapsed>s", proc.stderr)
    return {"stdout": proc.stdout, "stderr": stderr, "status": proc.returncode}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--stage-script", type=Path, default=HERE.parent / "stage_tests.py",
                        help="fixture stager from the revision being measured")
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("requires the original CPython 3.14.5 interpreter and test package")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    callers = original_callers()
    originals = [case for case in cases(load_suite(manifest["module"])) if case.id() in callers]
    if len(originals) != len(callers) or len(callers) != 20:
        raise ValueError("discovered cases do not match the twenty historical IDs")
    reference = unittest.TestResult()
    unittest.TestSuite(originals).run(reference)
    if (not reference.wasSuccessful() or reference.skipped or reference.testsRun != len(originals)):
        raise ValueError(f"original CPython results changed: {reference}")
    primary = json.loads((HERE / "native-linux.json").read_bytes())
    report = {"cpython_commit": manifest["commit"], "reference_passes": reference.testsRun,
              "unittest_denominator": primary[0]["denominator"], "compiler_sha256": file_sha(args.imre),
              "runtime_sha256": file_sha(args.runtime), "modes": {}, "fixtures": {}}
    with tempfile.TemporaryDirectory() as temporary:
        stage_root = Path(temporary)
        subprocess.run([sys.executable, "-I", "-B", args.stage_script.resolve(), stage_root], check=True)
        paths = sorted(path for path in stage_root.rglob("*") if path.is_file())
        report["staged_files"] = len(paths)
        digest = hashlib.sha256()
        for path in paths:
            digest.update(f"{path.relative_to(stage_root).as_posix()} {git_blob(path)}\n".encode())
        report["fixture_manifest_sha256"] = digest.hexdigest()
        root = stage_root / "Lib"
        report["support_blob"] = git_blob(root / "test/support/__init__.py")
        from test import support
        if git_blob(Path(support.__file__)) != report["support_blob"]:
            raise ValueError("CPython test.support differs from the staged fixture")
        logging_id = "test.test_logging.BasicConfigTest.test_strformatstyle"
        logging_case, = cases(unittest.defaultTestLoader.loadTestsFromName(logging_id))
        logging_path = root / "test/test_logging.py"
        if git_blob(Path(sys.modules[type(logging_case).__module__].__file__)) != git_blob(logging_path):
            raise ValueError("CPython logging fixture differs from the staged blob")
        logging_reference = unittest.TestResult()
        unittest.TestSuite([logging_case]).run(logging_reference)
        if logging_reference.testsRun != 1 or not logging_reference.wasSuccessful() or logging_reference.skipped:
            raise ValueError(f"{logging_id}: {logging_reference}")
        report["logging"] = {"id": logging_id, "reference": "pass", "source_blob": git_blob(logging_path),
                             "method_sha256": hashlib.sha256(method_bytes(
                                 logging_path, "BasicConfigTest", "test_strformatstyle")).hexdigest()}
        entries = fixture_entries(root)
        report["entry_sha256"] = {name: hashlib.sha256(source).hexdigest() for name, source in entries.items()}
        report["fixture_references"] = {name: reference_probe(source) for name, source in entries.items()}
        for reachable in (False, True):
            for group in primary:
                level = group["opt_level"]
                mode = ("reachable" if reachable else "ordinary") + f"-O{level}"
                tools = Toolchain(args.imre, args.runtime, args.clang, level, reachable, root)
                report["fixtures"][mode] = {name: fixture_probe(tools, root, source)
                                             for name, source in entries.items()}
                prior = {r["id"]: r for r in group["records"] if r["id"] in callers}
                rows = {}
                for case in originals:
                    result = try_default(case, prior[case.id()], manifest["files"], tools, root)
                    proof = result["evidence"]
                    if proof["method_sha256"] != callers[case.id()]:
                        raise ValueError(f"{case.id()}: original method changed")
                    row = dict(id=case.id(), occurrence=1, opt_level=level, reference="pass",
                               fixture_variant=prior[case.id()]["fixture_variant"], **result)
                    row["classification"] = classification(row)
                    validate_row(row, "pass", level, group["platform"])
                    rows[case.id()] = row
                report["modes"][mode] = rows
                print(mode, dict(Counter(r["classification"] for r in rows.values())),
                      Counter(r["evidence"].get("upstream_diagnostic") for r in rows.values()), flush=True)
    args.result.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
