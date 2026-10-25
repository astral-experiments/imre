"""Exercise complete, unmodified CPython copyfile methods at both optimization levels."""

import argparse
import ast
from contextlib import chdir
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

# An unexpectedly uncalled operation or a partially executed routine is a failure.
HOST_CASES = {
    "test_copyfile_copy_dir": 3,
    "test_copyfile_named_pipe": 2,
    "test_copyfile_nonexistent_dir": 1,
    "test_copyfile_same_file": 2,
    "test_copyfile_symlinks": 2,
    "test_dont_copy_file_onto_link_to_itself": 1,
    "test_dont_copy_file_onto_symlink_to_itself": 1,
}
NATIVE_CASE = "test_copyfile_return_value"

def copy_fixture(case):
    # Use BaseTest.mkdtemp, including its original cleanup, for each directory.
    src, dst = case.mkdtemp(), case.mkdtemp()
    return f'''import os.path
import shutil

def create_file(path: str, content: str) -> None:
    with open(path, "xb") as stream:
        stream.write(content.encode())

def read_file(path: str) -> str:
    with open(path, "r", encoding="utf-8") as stream:
        return stream.read()

class NativeCase:
    used_source: bool
    def __init__(self) -> None:
        self.used_source = False
    def mkdtemp(self) -> str:
        if self.used_source:
            return {dst!r}
        self.used_source = True
        return {src!r}
    def assertTrue(self, actual: bool) -> None:
        assert actual
    def assertEqual(self, actual: str, expected: str) -> None:
        assert actual == expected
'''


ERRORS = ("shutil.SameFileError", "shutil.SpecialFileError", "FileNotFoundError",
          "IsADirectoryError", "NotADirectoryError", "PermissionError", "FileExistsError",
          "ValueError", "OSError")


def driver():
    return '''import shutil
import sys

try:
    result = shutil.copyfile(sys.argv[1], sys.argv[2], follow_symlinks=sys.argv[3] == "1")
    print("ok")
    print(repr(result))
''' + "".join(f"except {exc} as error:\n    print('{exc}')\n    print(repr(str(error)))\n" for exc in ERRORS)


def run(command):
    result = subprocess.run(command, text=True, capture_output=True, timeout=180)
    if result.returncode:
        raise RuntimeError(f"{command[0]} exited {result.returncode}: {result.stderr}")
    return result


def build(args, directory, stem, source, level):
    folder = directory / stem
    folder.mkdir()
    return Toolchain(args.imre, args.runtime, args.clang, level).build(source, folder)


def cases(args, suite, directory, level):
    tools = Toolchain(args.imre, args.runtime, args.clang, level)
    test_id = suite.TestCopy(NATIVE_CASE).id()
    def native(case, original):
        folder = directory / "routine"
        folder.mkdir()
        execute = native_method(tools, folder, Path(suite.__file__), "TestCopy",
                                test_id, copy_fixture(case))
        return {"status": "native_body_host_fixture", **execute(case, original)}

    exe = build(args, directory, "copy", driver().encode(), level)
    import builtins
    errors = {n: getattr(suite.shutil if "." in n else builtins, n.split(".")[-1]) for n in ERRORS}

    def host(case, original):
        calls = 0
        def invoke(src, dst, *, follow_symlinks=True):
            nonlocal calls
            result = run([exe, src, dst, str(int(follow_symlinks))])
            if result.stderr:
                raise AssertionError(result.stderr)
            status, payload = result.stdout.rstrip("\n").split("\n", 1)
            value = ast.literal_eval(payload)
            calls += 1
            if status != "ok":
                raise errors[status](value)
            return value

        with patch.object(suite.shutil, "copyfile", invoke):
            original()
        assert calls == HOST_CASES[case._testMethodName], (case.id(), calls)
        if case._testMethodName.startswith("test_dont_copy_file_onto_"):
            return {"status": "mixed", "native_calls": calls, "native": "copyfile",
                    "host": "unittest assertions and shutil.rmtree in the method's finally block"}
        return {"status": "host_driven_native", "native_calls": calls}

    executions = {suite.TestCopy(name).id(): host for name in HOST_CASES}
    executions[test_id] = native
    evidence, failures, skips = run_in_fixtures(load_suite("test.test_shutil"), executions)
    rows = {name: values[0] for name, values in evidence.items() if values}
    rows.update({name: {"status": "failed", "diagnostics": reason} for name, reason in failures})
    rows.update({name: {"status": "reference_skip", "reason": reason} for name, reason in skips})
    assert rows.keys() == executions.keys()
    return rows


def main(run_cases=cases):
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + option, required=True, type=lambda p: Path(p).resolve())
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or os.name != "posix":
        parser.error("requires test-enabled CPython 3.14.5 on POSIX")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    import test
    test_root = STDLIB / "upstream/Lib/test"
    test.__path__.insert(0, str(test_root))
    report = {"cpython_commit": manifest["commit"], "optimization_levels": {}}
    with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
        from test import test_shutil as suite
        assert Path(suite.__file__).resolve() == test_root / "test_shutil.py"
        for level in (0, 3):
            directory = Path(tmp) / str(level)
            directory.mkdir()
            with chdir(directory):
                report["optimization_levels"][level] = run_cases(args, suite, directory, level)
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(v["status"] == "failed" for rows in report["optimization_levels"].values()
                   for v in rows.values()))


if __name__ == "__main__":
    sys.exit(main())
