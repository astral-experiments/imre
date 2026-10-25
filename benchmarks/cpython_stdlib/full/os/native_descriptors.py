"""Compile complete guarded CPython OS test methods with native descriptor fixtures."""

import argparse
import ast
from contextlib import chdir
import hashlib
import json
import importlib
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files
from suite_native import Toolchain, method_bytes, run_in_fixtures

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

TESTFN = {testfn!r}

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
    # test_os probes ACLs, links and extended attributes while importing.
    # Separate concurrent runs even before unittest starts its first fixture.
    with tempfile.TemporaryDirectory() as work, chdir(work):
        return import_sources()


def import_sources():
    import test

    modules = {}
    for suite in ("os", "posix"):
        manifest = json.loads((HERE / f"{suite}.json").read_text())
        verify_files(HERE / "upstream", manifest["files"])
        verify_files(Path(test.__file__).resolve().parent,
                     {path.removeprefix("Lib/test/"): blob for path, blob in manifest["files"].items()
                      if path.startswith("Lib/test/")})
        modules["test_" + suite] = importlib.import_module(manifest["module"])
    return modules, manifest["commit"]


def host_fixtures(case):
    if case.__class__.__name__ == "PosixTester":
        return ["PosixTester.setUp and cleanups", "test_posix.tearDownModule"]
    return (["FileTests.setUp and tearDown"] if case.__class__.__name__ == "FileTests" else []) + [
        "test_os.tearDownModule"]


def descriptor_suite(module, selections):
    evidence, failures, skips = run_in_fixtures(load_suite(module.__name__), selections)
    rows = {}
    for name, entries in evidence.items():
        if len(entries) != 1:
            raise ValueError(f"unexpected repeated descriptor ID: {name}")
        rows[name], = entries
    for name, diagnostic in failures:
        rows[name] = {"status": "failed", "diagnostic": diagnostic}
    for name, reason in skips:
        rows[name] = {"reference_skip": reason}
    missing = selections.keys() - rows.keys()
    if missing:
        raise ValueError(f"descriptor methods did not run: {sorted(missing)}; failures={failures}")
    return rows


def compile_descriptor(case, tools, root, count):
    from test.support import os_helper

    cls, method = type(case).__name__, case._testMethodName
    path = Path(sys.modules[type(case).__module__].__file__)
    source = path.read_bytes()
    klass, = [n for n in ast.parse(source).body if isinstance(n, ast.ClassDef) and n.name == cls]
    definition, = [n for n in klass.body if isinstance(n, ast.FunctionDef) and n.name == method]
    for node in ast.walk(definition):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "addCleanup":
            if len(node.args) != 2 or ast.unparse(node.args[0]) != "os.close":
                raise ValueError(f"unexpected cleanup in {case.id()}")
    if definition.decorator_list:
        # These release guards set unittest's skip flags. run_in_fixtures
        # checks those flags before this callback compiles or runs anything.
        for guard in definition.decorator_list:
            if not isinstance(guard, ast.Call) or ast.unparse(guard.func) not in {
                "unittest.skipIf", "unittest.skipUnless",
                "support.requires_linux_version", "support.requires_subprocess",
            }:
                raise ValueError(f"unhandled guard on {case.id()}: {ast.unparse(guard)}")
        body = b"".join(source.splitlines(keepends=True)[definition.lineno - 1:definition.end_lineno])
    else:
        body = method_bytes(path, cls, method)
    wd = root / case.id()
    wd.mkdir()
    (wd / "subject.py").write_text(SUBJECT)
    (wd / "os_helper.py").write_text(BAD_FD.format(testfn=os_helper.TESTFN))
    adapter = ADAPTER
    if method == "test_dup2":
        adapter = adapter.replace("actual: bool, expected: bool", "actual: int, expected: int")
    entry = adapter.encode() + body + f"""
case = Case()
try:
    case.{method}()
finally:
    case.clean(os.close)
print(case.checks)
""".encode()
    try:
        binary = tools.build(entry, wd)
    except RuntimeError as error:
        return {"status": "blocked", "diagnostic": str(error).replace(str(root), "<fixture>")}
    # The child inherits the upstream fixture's current working directory.
    run = subprocess.run([binary], capture_output=True, text=True, timeout=30)
    if run.returncode or run.stderr or run.stdout != f"{count}\n":
        raise AssertionError(f"native exit {run.returncode}: {run.stdout!r} {run.stderr!r}")
    return {"method_sha256": hashlib.sha256(body).hexdigest(), "status": "native_body_host_fixture",
            "host_fixtures": host_fixtures(case), "assertions": count}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    modules, commit = verify_sources(args.cpython_source)
    tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), int(args.opt_level))
    reports = {}
    with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
        root = Path(tmp)
        for module in modules.values():
            selections = {}
            for name, count in CASES.items():
                cls, method = name.split(".")
                if hasattr(module, cls):
                    case = getattr(module, cls)(method)
                    selections[case.id()] = lambda case, _, n=count: compile_descriptor(case, tools, root, n)
            reports.update(descriptor_suite(module, selections))
    for name, row in reports.items():
        print(name, row, flush=True)
    args.result.write_text(json.dumps({"cpython_commit": commit, "opt_level": int(args.opt_level),
                                      "cases": reports}, indent=2) + "\n")
    return int(any(row.get("status") in {"blocked", "failed"} for row in reports.values()))


if __name__ == "__main__":
    sys.exit(main())
