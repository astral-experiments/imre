"""Run the unchanged module-name method using the two builtin classes it asserts."""

import re
import subprocess

from suite_inventory import verify_files
from suite_native import NativeCompileError, native_method, testcase_fixture

CASE_ID = "test.test_sys.SysModuleTest.test_module_names"


def verify_table(cpython_source, native_table):
    header = "Python/stdlib_module_names.h"
    verify_files(cpython_source, {header: "56e349a544c07971287625a740eb9f5dd566bdc6"})
    names = re.findall(r'^"([^"]+)",?$', (cpython_source / header).read_text(), re.MULTILINE)
    if len(names) != 297 or names != sorted(set(names)) or names != native_table.read_text().splitlines():
        raise ValueError("native module names differ from the pinned CPython table")


FIXTURE = """import sys

class NativeCase:
    def assertIsInstance(self, value: frozenset[str] | str, cls: type) -> None:
        if cls is frozenset:
            if isinstance(value, frozenset):
                return
        elif cls is str:
            if isinstance(value, str):
                return
        else:
            raise ValueError("unsupported fixture class")
        raise AssertionError("value has the wrong class")
"""


def execution(toolchain, work, source):
    args = (toolchain, work, source, "SysModuleTest", CASE_ID)
    operation = ("imre", "read the singleton sys.stdlib_module_names, check its type and iterate all elements")
    try:
        return native_method(*args, "import sys\n" + testcase_fixture(), operation=operation)
    except NativeCompileError as exc:
        match = re.search(r"case\.py:(\d+):(\d+): Imre: unknown class method\s*$", str(exc))
        if not match:
            raise
        line, column = map(int, match.groups())
        entry = (work / "case.py").read_text().splitlines()
        if not entry[line - 1][column - 1:].startswith("self.assertIsInstance("):
            raise

    controls = work / "assertion-controls"
    controls.mkdir()
    checks = """
case = NativeCase()
try:
    case.assertIsInstance("sys", frozenset)
except AssertionError:
    print("string rejected")
try:
    case.assertIsInstance(sys.stdlib_module_names, str)
except AssertionError:
    print("frozenset rejected")
"""
    binary = toolchain.build((FIXTURE + checks).encode(), controls)
    result = subprocess.run([binary], capture_output=True, timeout=20, check=True)
    if result.stdout != b"string rejected\nfrozenset rejected\n" or result.stderr:
        raise AssertionError((result.stdout, result.stderr))
    run = native_method(*args, FIXTURE, operation=operation)

    def execute(case, original):
        record = run(case, original)
        record["observation"]["evidence"].update(
            assertions="Native isinstance checks for both builtin class operands; both swapped arguments raise AssertionError",
            shared_assertion_blocker="unittest.TestCase.assertIsInstance: unknown class method",
        )
        return record

    return execute
