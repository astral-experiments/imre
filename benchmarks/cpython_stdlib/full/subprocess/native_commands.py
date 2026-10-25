"""Compile command and wait tests with the actual command selected by setUpModule."""

import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures, testcase_fixture

METHODS = {
    "ProcessTestCase": {
        "test_call_seq": "call and wait for a sequence command",
        "test_call_kwargs": "call and wait with a replacement environment",
        "test_check_call_zero": "check_call on the zero-return fixture command",
        "test_wait": "wait twice on the zero-return fixture command",
    },
    "RunFuncTestCase": {"test_check_zero": "run the fixture command with check=True and observe its return code"},
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    if sys.version_info[:3] != (3, 14, 5):
        raise ValueError("use the pinned CPython 3.14.5 build")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tools = Toolchain(args.imre, args.runtime, args.clang, int(args.opt_level))
    source = HERE / "upstream/Lib/test/test_subprocess.py"
    from test import test_subprocess

    def execute(case, original):
        # This callback runs after the original setUpModule has selected and
        # executed its probe. Preserve the actual type as well as the elements.
        command = test_subprocess.ZERO_RETURN_CMD
        if type(command) is not tuple or any(type(arg) is not str for arg in command):
            raise ValueError("unsupported fixture command")
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "command_fixture.py").write_text(f"ZERO_RETURN_CMD = {command!r}\n")
            fixture = "import os\nimport subprocess\nimport sys\nfrom command_fixture import ZERO_RETURN_CMD\n" + testcase_fixture()
            cls = type(case).__name__
            run = native_method(tools, Path(directory), source, cls, case.id(), fixture,
                                operation=("imre", METHODS[cls][case._testMethodName]))
            evidence = run(case, original)
        evidence["child"] = "CPython -c helper" if case._testMethodName in {"test_call_seq", "test_call_kwargs"} else "setUpModule ZERO_RETURN_CMD"
        return evidence

    wanted = {f"test.test_subprocess.{cls}.{m}": execute for cls, methods in METHODS.items() for m in methods}
    with patch.dict(os.environ, IMRE_PYTHON=sys.executable):
        results, failures, skips = run_in_fixtures(load_suite(manifest["module"]), wanted)
    if failures or skips or results.keys() != wanted.keys():
        raise AssertionError((failures, skips, wanted.keys() - results.keys()))
    args.result.write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
