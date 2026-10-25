"""Run unchanged annotationlib methods used to verify the shared dependency."""

import argparse
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures, testcase_fixture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--forward-refs", action="store_true",
                        help="verify the three ForwardRef argument errors with native assertRaises")
    args = parser.parse_args()
    manifest = json.loads((HERE / "ANNOTATIONLIB.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    if args.forward_refs:
        owner, symbol = "TestForwardRefClass", "ForwardRef"
        methods = ["test_forwardref_only_str_arg", "test_forwardref_instance_type_error",
                   "test_forwardref_subclass_type_error"]
    else:
        owner, symbol, methods = "TestFormat", "Format", ["test_enum"]
    path = HERE / "upstream/Lib/test/test_annotationlib.py"
    report = {}
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        with TemporaryDirectory() as directory:
            fixture = f"from annotationlib import {symbol}\n" + testcase_fixture()
            executions = {}
            for method in methods:
                case_id = f'{manifest["module"]}.{owner}.{method}'
                work = Path(directory) / method
                work.mkdir()
                executions[case_id] = native_method(tools, work, path, owner, case_id, fixture)
            evidence, failures, skips = run_in_fixtures(load_suite(manifest["module"]), executions)
            if failures or skips or any(len(evidence.get(k, ())) != 1 for k in executions):
                raise RuntimeError((failures, skips, evidence))
            report[f"O{level}"] = evidence
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
