"""Run the unchanged annotation-format method used to verify the shared dependency."""

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
    args = parser.parse_args()
    manifest = json.loads((HERE / "ANNOTATIONLIB.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    case_id = manifest["module"] + ".TestFormat.test_enum"
    path = HERE / "upstream/Lib/test/test_annotationlib.py"
    report = {}
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        with TemporaryDirectory() as directory:
            fixture = "from annotationlib import Format\n" + testcase_fixture()
            run = native_method(tools, Path(directory), path, "TestFormat", case_id, fixture)
            evidence, failures, skips = run_in_fixtures(load_suite(manifest["module"]), {case_id: run})
            if failures or skips or len(evidence.get(case_id, ())) != 1:
                raise RuntimeError((failures, skips, evidence))
            report[f"O{level}"] = evidence
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
