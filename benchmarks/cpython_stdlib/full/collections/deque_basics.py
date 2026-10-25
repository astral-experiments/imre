"""Run the original deque test and prove its last list-order assertion detects a broken reversal."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from routines import native_fixture
from suite_inventory import load_suite, verify_files
from suite_native import Toolchain, native_method, run_in_fixtures

CASE = "test.test_deque.TestBasic.test_basics"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--compiler-revision", required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use a test-enabled CPython 3.14.5")
    from test import test_deque

    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test_deque.__file__).resolve().parents[2], manifest["files"])
    path = HERE / "upstream/Lib/test/test_deque.py"
    _, fixture = native_fixture(path, "TestBasic", "test_basics", test_deque.TestBasic("test_basics"))
    tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), args.opt_level)
    with tempfile.TemporaryDirectory() as temp:
        directory = Path(temp)
        execute = native_method(tools, directory, path, "TestBasic", CASE, fixture,
                                operation=("imre", "Reinitialize, grow, iterate and pop a native deque; reverse the list of right pops."))
        observed, failures, skips = run_in_fixtures(load_suite("test.test_deque"), {CASE: execute})
        if failures or skips or set(observed) != {CASE}:
            raise ValueError((failures, skips, observed))
        control = {"result": "unavailable", "reason": "the link-time negative control uses GNU --wrap"}
        if sys.platform.startswith("linux"):
            noop = directory / "noop.c"
            noop.write_text("void __wrap_imre_list_reverse(void *list) {(void)list;}\n")
            binary = directory / "case-no-reverse"
            subprocess.run([tools.imre, "build", directory / "case.py", "--no-cache",
                            "--runtime", tools.runtime, "--clang", tools.clang, "--opt-level", str(tools.level),
                            "-C", "link-arg=-Wl,--wrap=imre_list_reverse", "-C", f"link-arg={noop}", "-o", binary],
                           check=True, capture_output=True, timeout=180)
            wrong = subprocess.run([binary], capture_output=True, text=True, timeout=60)
            if (not wrong.returncode or wrong.stdout or "AssertionError" not in wrong.stderr
                    or "First differing element 0:\n399\n150" not in wrong.stderr):
                raise ValueError(f"wrong-order native control did not fail the original assertion: {wrong}")
            control = {"result": "failed-as-expected", "wrapped": "imre_list_reverse",
                       "exit": wrong.returncode, "stderr": wrong.stderr.replace(temp, "<work>")}
    report = {"cpython_commit": manifest["commit"], "compiler_revision": args.compiler_revision,
              "opt_level": tools.level, "tests": observed, "negative_control": control}
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
