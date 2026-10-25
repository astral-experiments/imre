"""Execute complete Counter methods with native assertions under CPython's fixtures."""

import argparse
import json
from pathlib import Path
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1]))
from native_counter import METHODS, assertion_fixture
from routines import COUNTER_OPERATIONS
from suite_inventory import load_suite, verify_files
from suite_native import Toolchain, native_method, run_in_fixtures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--opt-level", choices=(0, 3), type=int, required=True)
    parser.add_argument("--compiler-revision", required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use test-enabled CPython 3.14.5")
    import test

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], manifest["files"])
    suite = load_suite("test.test_collections")
    tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    source = HERE / "upstream/Lib/test/test_collections.py"
    with tempfile.TemporaryDirectory() as tmp:
        executors = {}
        for method in METHODS:
            case_id = f"test.test_collections.TestCounter.{method}"
            directory = Path(tmp) / method
            directory.mkdir()
            executors[case_id] = native_method(
                tools, directory, source, "TestCounter", case_id, assertion_fixture(method, "NativeCase"),
                operation=("imre", COUNTER_OPERATIONS[method]), assertions="imre")
        evidence, failures, skips = run_in_fixtures(suite, executors)
    report = {"cpython_commit": manifest["commit"], "compiler_revision": args.compiler_revision, "opt_level": args.opt_level,
              "operation": "native Imre Counter", "tests": evidence,
              "failures": failures, "skips": skips}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(bool(failures or skips) or evidence.keys() != executors.keys())


if __name__ == "__main__":
    sys.exit(main())
