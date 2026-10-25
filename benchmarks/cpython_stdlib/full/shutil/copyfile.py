"""Exercise complete, unmodified CPython copyfile methods at both optimization levels."""

import argparse
import ast
from contextlib import chdir
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from upstream import verify_upstream

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

FIXTURE = '''import os.path
import shutil
import sys

def create_file(path: str, content: str) -> None:
    with open(path, "xb") as stream:
        stream.write(content.encode())

def read_file(path: str) -> str:
    with open(path, "r", encoding="utf-8") as stream:
        return stream.read()

class Case:
    next_dir: int
    checks: int
    def __init__(self) -> None:
        self.next_dir = 1
        self.checks = 0

    def mkdtemp(self) -> str:
        directory = sys.argv[self.next_dir]
        self.next_dir += 1
        return directory

    def assertTrue(self, actual: bool) -> None:
        assert actual
        self.checks += 1

    def assertEqual(self, actual: str, expected: str) -> None:
        assert actual == expected
        self.checks += 1

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


def native_method(path):
    data = path.read_bytes()
    cls = next(n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == "TestCopy")
    method = next(n for n in cls.body if getattr(n, "name", None) == NATIVE_CASE)
    assert not method.decorator_list
    return b"".join(data.splitlines(keepends=True)[method.lineno - 1:method.end_lineno])


def run(command):
    result = subprocess.run(command, text=True, capture_output=True, timeout=180)
    if result.returncode:
        raise RuntimeError(f"{command[0]} exited {result.returncode}: {result.stderr}")
    return result


def build(args, directory, stem, source, level):
    path = directory / (stem + ".py")
    path.write_bytes(source)
    exe = directory / stem
    run([args.imre, "build", path, "--no-cache", "--opt-level", str(level), "--clang",
         args.clang, "--runtime", args.runtime, "-o", exe])
    return exe


def cases(args, suite, directory, level):
    body = native_method(Path(suite.__file__))
    native = build(args, directory, "routine", FIXTURE.encode() + body
                   + f"\ncase = Case()\ncase.{NATIVE_CASE}()\nprint(case.checks)\n".encode(), level)
    src, dst = directory / "source", directory / "destination"
    src.mkdir()
    dst.mkdir()
    result = run([native, src, dst])
    if result.stderr or result.stdout != "2\n":
        raise AssertionError((result.stdout, result.stderr))
    rows = {suite.TestCopy(NATIVE_CASE).id(): {"status": "fully_native",
            "method_sha256": hashlib.sha256(body).hexdigest(), "assertions": 2}}

    exe = build(args, directory, "copy", driver().encode(), level)
    import builtins
    errors = {n: getattr(suite.shutil if "." in n else builtins, n.split(".")[-1]) for n in ERRORS}
    for method, expected in HOST_CASES.items():
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

        case = suite.TestCopy(method)
        outcome = unittest.TestResult()
        with patch.object(suite.shutil, "copyfile", invoke):
            case.run(outcome)
        if outcome.skipped:
            rows[case.id()] = {"status": "reference_skip", "reason": outcome.skipped[0][1]}
        elif outcome.wasSuccessful() and outcome.testsRun == 1 and calls == expected:
            rows[case.id()] = {"status": "host_driven_native", "native_calls": calls}
        else:
            rows[case.id()] = {"status": "failed", "native_calls": calls,
                "diagnostics": [reason for _, reason in outcome.errors + outcome.failures]}
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + option, required=True, type=lambda p: Path(p).resolve())
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or os.name != "posix":
        parser.error("requires test-enabled CPython 3.14.5 on POSIX")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    test_root = STDLIB / "upstream/Lib/test"
    test.__path__.insert(0, str(test_root))
    report = {"cpython_commit": verify_upstream(), "optimization_levels": {}}
    with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
        from test import test_shutil as suite
        assert Path(suite.__file__).resolve() == test_root / "test_shutil.py"
        for level in (0, 3):
            directory = Path(tmp) / str(level)
            directory.mkdir()
            with chdir(directory):
                report["optimization_levels"][level] = cases(args, suite, directory, level)
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(v["status"] == "failed" for rows in report["optimization_levels"].values()
                   for v in rows.values()))


if __name__ == "__main__":
    sys.exit(main())
