"""Run complete deque queue, stack, and indexed assignment tests under the original fixtures."""

import argparse
import json
from pathlib import Path
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files
from suite_native import Toolchain, native_method, run_in_fixtures


BIG_METHODS = ("test_big_queue_popleft", "test_big_queue_popright",
               "test_big_stack_left", "test_big_stack_right")
METHODS = (*BIG_METHODS, "test_setitem")
BIG_OPERATION = ("imre", "Construct, append, and pop at opposite or matching deque ends")


def big_fixture(size):
    return f"""from collections import deque
BIG = {size}

class NativeCase:
    def assertEqual(self, first: int, second: int) -> None:
        if first != second:
            raise AssertionError(f"{{first}} != {{second}}")
"""


def method_fixture(method, size):
    if method in BIG_METHODS:
        return big_fixture(size), BIG_OPERATION
    if method != "test_setitem":
        raise ValueError(f"unknown deque method: {method}")
    return """from collections import deque

class NativeCase:
    def assertEqual(self, first: list[int], second: list[int]) -> None:
        if len(first) != len(second):
            raise AssertionError("list lengths differ")
        for index in range(len(first)):
            if first[index] != second[index]:
                raise AssertionError("list items differ")
""", ("imre", "Construct, index-assign, and iterate a native deque")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", choices=(0, 3), type=int, required=True)
    parser.add_argument("--compiler-revision")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use a test-enabled CPython 3.14.5")
    from test import test_deque

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test_deque.__file__).resolve().parents[2], manifest["files"])
    source = HERE / "upstream/Lib/test/test_deque.py"
    tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    suite = load_suite("test.test_deque")
    with tempfile.TemporaryDirectory() as tmp:
        executors = {}
        for method in METHODS:
            case_id = f"test.test_deque.TestBasic.{method}"
            directory = Path(tmp) / method
            directory.mkdir()
            fixture, operation = method_fixture(method, test_deque.BIG)
            executors[case_id] = native_method(tools, directory, source, "TestBasic", case_id, fixture,
                                               operation=operation)
        evidence, failures, skips = run_in_fixtures(suite, executors)
    report = {"cpython_commit": manifest["commit"], "opt_level": args.opt_level,
              "operation": "native Imre deque", "tests": evidence,
              "failures": failures, "skips": skips}
    if args.compiler_revision:
        report["compiler_revision"] = args.compiler_revision
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(bool(failures or skips) or evidence.keys() != executors.keys())


if __name__ == "__main__":
    sys.exit(main())
