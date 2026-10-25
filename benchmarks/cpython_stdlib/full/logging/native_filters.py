"""Execute the complete CPython BasicFilterTest method under its original fixtures."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files
from suite_native import Toolchain, native_method, run_in_fixtures, testcase_fixture


def filter_cases(toolchain, work):
    case_id = "test.test_logging.BasicFilterTest.test_empty_filter"
    callback = native_method(toolchain, work, STDLIB / "upstream/Lib/test/test_logging.py",
                             "BasicFilterTest", case_id, "import logging\n" + testcase_fixture(),
                             operation=("imre", "Native Filter, makeLogRecord and name predicate"))
    return {case_id: callback}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("use a test-enabled CPython 3.14.5 with assertions enabled")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], manifest["files"] | manifest["reference_files"])
    tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    suite = load_suite(manifest["module"])
    denominator = len(list(cases(suite)))
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        sample = HERE / "filter_values.py"
        program = tools.build(sample.read_bytes(), work)
        def output(cmd):
            run = subprocess.run(cmd, capture_output=True, timeout=30, check=True)
            return run.stdout, run.stderr
        if output([program]) != output([sys.executable, sample]):
            raise AssertionError("native logger-name filtering differs from CPython")
        callbacks = filter_cases(tools, work)
        observed, failures, skips = run_in_fixtures(suite, callbacks)
        if failures or skips or set(observed) != set(callbacks):
            raise AssertionError((failures, skips, observed))
    args.result.write_text(json.dumps({"denominator": denominator, "opt_level": args.opt_level,
                                       "evidence": observed}, indent=2) + "\n")
    print(f"-O{args.opt_level}: native body, operation and assertion; original CPython hooks")


if __name__ == "__main__":
    main()
