"""Run complete deque queue/stack tests under their original CPython fixtures."""

import argparse
import json
from pathlib import Path
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files
from suite_native import Toolchain, native_method, run_in_fixtures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", choices=(0, 3), type=int, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use a test-enabled CPython 3.14.5")
    from test import test_deque

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test_deque.__file__).resolve().parents[2], manifest["files"])
    source = HERE / "upstream/Lib/test/test_deque.py"
    fixture = f"""from collections import deque
BIG = {test_deque.BIG}

class NativeCase:
    def assertEqual(self, first: int, second: int) -> None:
        if first != second:
            raise AssertionError(f"{{first}} != {{second}}")
"""
    tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    suite = load_suite("test.test_deque")
    with tempfile.TemporaryDirectory() as tmp:
        executors = {}
        for method in ("test_big_queue_popleft", "test_big_queue_popright",
                       "test_big_stack_left", "test_big_stack_right", "test_setitem"):
            case_id = f"test.test_deque.TestBasic.{method}"
            directory = Path(tmp) / method
            directory.mkdir()
            if method == "test_setitem":
                prefix = """from collections import deque

class NativeCase:
    def assertEqual(self, first: list[int], second: list[int]) -> None:
        if len(first) != len(second):
            raise AssertionError("list lengths differ")
        for index in range(len(first)):
            if first[index] != second[index]:
                raise AssertionError("list items differ")
"""
                operation = "Construct, index-assign, and iterate a native deque"
            else:
                prefix = fixture
                operation = "Construct, append, and pop at opposite or matching deque ends"
            executors[case_id] = native_method(tools, directory, source, "TestBasic", case_id, prefix,
                                               operation=("imre", operation))
        evidence, failures, skips = run_in_fixtures(suite, executors)
    report = {"cpython_commit": manifest["commit"], "opt_level": args.opt_level,
              "operation": "native Imre deque", "tests": evidence,
              "failures": failures, "skips": skips}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(bool(failures or skips) or evidence.keys() != executors.keys())


if __name__ == "__main__":
    sys.exit(main())
