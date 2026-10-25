"""Compile complete guarded CPython OS test methods with native descriptor fixtures."""

import argparse
import ast
import hashlib
import json
import importlib
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
CASES = {
    "FDInheritanceTests.test_open": 1,
    "FDInheritanceTests.test_get_set_inheritable": 2,
    "FDInheritanceTests.test_dup": 1,
    "FDInheritanceTests.test_dup2": 4,
    "FDInheritanceTests.test_pipe": 2,
    "FDInheritanceTests.test_dup_standard_stream": 1,
    "FileTests.test_open_keywords": 0,
    "BlockingTests.test_blocking": 3,
    "TestInvalidFD.test_isatty": 1,
    "PosixTester.test_oscloexec": 1,
}

# All cleanup registrations in these methods use the same os.close callback.
ADAPTER = """from typing import Callable
import subject as os
import os_helper

class Case:
    checks: int
    cleanups: list[int]

    def __init__(self) -> None:
        self.checks = 0
        self.cleanups = []

    def addCleanup(self, fn: Callable[[int], None], fd: int) -> None:
        self.cleanups.append(fd)

    def clean(self, fn: Callable[[int], None]) -> None:
        while self.cleanups:
            fn(self.cleanups.pop())

    def assertEqual(self, actual: bool, expected: bool) -> None:
        assert actual == expected
        self.checks += 1

    def assertGreater(self, actual: int, bound: int) -> None:
        assert actual > bound
        self.checks += 1

    def assertTrue(self, actual: bool) -> None:
        assert actual
        self.checks += 1

    def assertFalse(self, actual: bool) -> None:
        assert not actual
        self.checks += 1

"""

# The compiler can pass this first-party function as a cleanup callback.
# Every descriptor operation, including the callback, calls the public os API.
SUBJECT = """import os
from os import O_RDONLY, O_CLOEXEC, open, dup, dup2, pipe, get_blocking, set_blocking, get_inheritable, set_inheritable, isatty

def close(fd: int) -> None:
    os.close(fd)
"""
BAD_FD = """import os

TESTFN = "fixture"

def make_bad_fd() -> int:
    fd = os.open(__file__, os.O_RDONLY)
    os.close(fd)
    return fd
"""


def verify_sources(source=None):
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise ValueError("run with test-enabled CPython 3.14.5")
    if source:
        sys.path.insert(0, str(source.resolve() / "Lib"))
    modules = {}
    for suite in ("os", "posix"):
        manifest = json.loads((HERE / f"{suite}.json").read_text())
        for filename, expected in manifest["files"].items():
            data = (HERE / "upstream" / filename).read_bytes()
            if hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest() != expected:
                raise ValueError(f"modified upstream file: {filename}")
            if filename.startswith("Lib/test/"):
                name = Path(filename).stem
                module = importlib.import_module("test." + name)
                if Path(module.__file__).read_bytes() != data:
                    raise ValueError(f"modified oracle file: {filename}")
                modules[name] = module
    return modules, manifest["commit"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    modules, commit = verify_sources(args.cpython_source)
    reports = {}
    cmd = [args.imre.resolve(), "build", "case.py", "--no-cache", "--opt-level", args.opt_level,
           "--runtime", args.runtime.resolve(), "--clang", args.clang.resolve(), "-o", "case"]
    with tempfile.TemporaryDirectory() as tmp:
        for name, count in CASES.items():
            cls, method = name.split(".")
            module = modules["test_posix" if cls == "PosixTester" else "test_os"]
            source = Path(module.__file__).read_bytes()
            lines = source.splitlines(keepends=True)
            classes = {node.name: node for node in ast.parse(source).body if isinstance(node, ast.ClassDef)}
            case = getattr(module, cls)(method)
            # Evaluate the real release decorators with the reference interpreter.
            for guarded in (type(case), getattr(case, method)):
                if getattr(guarded, "__unittest_skip__", False):
                    reports[case.id()] = {"reference_skip": guarded.__unittest_skip_why__}
                    break
            else:
                definition = next(n for n in classes[cls].body if isinstance(n, ast.FunctionDef) and n.name == method)
                for n in ast.walk(definition):
                    if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == "addCleanup":
                        if len(n.args) != 2 or ast.unparse(n.args[0]) != "os.close":
                            raise ValueError(f"unexpected cleanup in {name}")
                body = b"".join(lines[definition.lineno - 1:definition.end_lineno])
                wd = Path(tmp) / name
                wd.mkdir()
                (wd / "fixture").touch()
                (wd / "subject.py").write_text(SUBJECT)
                (wd / "os_helper.py").write_text(BAD_FD)
                adapter = ADAPTER.replace("actual: bool, expected: bool", "actual: int, expected: int") if method == "test_dup2" else ADAPTER
                (wd / "case.py").write_bytes(adapter.encode() + body + f"""
case = Case()
try:
    case.{method}()
finally:
    case.clean(os.close)
print(case.checks)
""".encode())
                entry = {"method_sha256": hashlib.sha256(body).hexdigest()}
                build = subprocess.run(cmd, cwd=wd, capture_output=True, text=True, timeout=120)
                if build.returncode:
                    entry.update(status="blocked", diagnostic=build.stderr.replace(str(wd), "<fixture>"))
                else:
                    run = subprocess.run([wd / "case"], cwd=wd, capture_output=True, text=True, timeout=30)
                    if run.returncode == 0 and not run.stderr and run.stdout == f"{count}\n":
                        entry.update(status="fully_native", assertions=count)
                    else:
                        entry.update(status="failed", returncode=run.returncode, stdout=run.stdout, stderr=run.stderr)
                reports[case.id()] = entry
                print(case.id(), entry, flush=True)
    args.result.write_text(json.dumps({"cpython_commit": commit, "opt_level": int(args.opt_level),
                                      "cases": reports}, indent=2) + "\n")
    return int(any(row.get("status") in {"blocked", "failed"} for row in reports.values()))


if __name__ == "__main__":
    sys.exit(main())
