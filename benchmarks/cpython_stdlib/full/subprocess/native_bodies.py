"""Run unchanged native subprocess methods and assertions under CPython fixtures."""

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

# The subprocess operation, method body, and assertions all run in the binary.
PROCESS_CASE = "test.test_subprocess.ProcessTestCase."
EPIPE_CASES = {PROCESS_CASE + "test_communicate_epipe", PROCESS_CASE + "test_communicate_epipe_only_stdin"}
NONBYTE_VIEW_CASE = PROCESS_CASE + "test_communicate_memoryview_input_nonbyte"
FD_LEAK_CASE = PROCESS_CASE + "test_communicate_pipe_fd_leak"
PIPE_BUF_CASE = PROCESS_CASE + "test_communicate_pipe_buf"
ZERO_CMD_CASES = EPIPE_CASES | {FD_LEAK_CASE}
PIPE_FIXTURE_CASES = ZERO_CMD_CASES | {PIPE_BUF_CASE, NONBYTE_VIEW_CASE, PROCESS_CASE + "test_communicate_memoryview_input"}
METHODS = {
    "ContextManagerTests": {
        "test_returncode": "int",
        "test_pipe": "bytes",
        "test_communicate_stdin": "int",
    },
    "ProcessTestCase": {
        "test_stdin_pipe": "int",
        "test_stdout_pipe": "bytes",
        "test_stderr_pipe": "bytes",
        "test_stdout_stderr_pipe": "bytes",
        "test_communicate_stdin": "int",
        "test_communicate_memoryview_input": None,
        "test_communicate_memoryview_input_nonbyte": None,
        "test_communicate_epipe": None,
        "test_communicate_epipe_only_stdin": None,
        "test_communicate_pipe_buf": None,
        "test_communicate_pipe_fd_leak": None,
        "test_communicate_stdout": "bytes",
        "test_communicate_stderr": "bytes",
        "test_communicate_returns": "bytes",
        "test_communicate_timeout": None,
        "test_communicate_timeout_large_output": None,
        "test_post_timeout_communicate_sends_input": "int",
    },
}
PREAMBLE = """import subprocess
import sys

class NativeCase:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def assertTrue(self, value: bool, msg: str = "assertTrue") -> None:
        if not value:
            raise ValueError(msg)
        self.checks += 1

    def assertEqual(self, left: TYPE | None, right: TYPE | None, msg: str = "assertEqual") -> None:
        if left is None:
            self.assertTrue(right is None, msg)
        elif right is None:
            self.fail(msg)
        else:
            self.assertTrue(left == right, msg)

    def assertStartsWith(self, value: str, prefix: str) -> None:
        self.assertTrue(value.startswith(prefix))

    def assertIn(self, part: str, value: str) -> None:
        self.assertTrue(part in value)

    def fail(self, msg: str) -> None:
        raise ValueError(msg)

"""



def subprocess_method(tools, directory, source, cls, case_id, fixture):
    if case_id not in PIPE_FIXTURE_CASES:
        return native_method(tools, directory, source, cls, case_id, fixture)
    preamble = "from importlib import import_module\nimport sys\n"
    setup = 'subprocess = import_module("subprocess")\n\n'
    args = None
    if case_id in ZERO_CMD_CASES:
        from test import test_subprocess
        # setUpModule can replace the fallback Python command with a checked executable.
        args = lambda: test_subprocess.ZERO_RETURN_CMD
        setup += "ZERO_RETURN_CMD = tuple(sys.argv[1:])\n\n"
    elif case_id == NONBYTE_VIEW_CASE:
        preamble += "import select\n"
    elif case_id == PIPE_BUF_CASE:
        from test import support
        preamble += "import os\n"
        setup += f"class support:\n    PIPE_MAX_SIZE = {support.PIPE_MAX_SIZE!r}\n"

    flags = ["--opt-level", str(tools.level), "--runtime", str(tools.runtime.resolve()),
             "--clang", str(tools.clang.resolve())]
    child_env = {"IMRE_PYTHON": str(tools.imre.resolve()), "IMRE_ENCODED_FLAGS": "\x1f".join(flags)}
    return native_method(tools, directory, source, cls, case_id,
                         preamble + testcase_fixture(setup=setup), cleanups=case_id != FD_LEAK_CASE, extra_env=child_env,
                         assertions="none" if case_id in EPIPE_CASES else "imre", fixture_args=args)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    parser.add_argument("--case", choices=[f"test.test_subprocess.{cls}.{method}"
                                           for cls, methods in METHODS.items() for method in methods],
                        help="Build and run only this registered original under its upstream fixtures.")
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    source = HERE / "upstream/Lib/test/test_subprocess.py"
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    if sys.version_info[:3] != (3, 14, 5):
        raise RuntimeError("use the pinned CPython 3.14.5 build")
    tools = Toolchain(args.imre, args.runtime, args.clang, int(args.opt_level))
    executions = {}
    with tempfile.TemporaryDirectory() as tmp:
        for cls, methods in METHODS.items():
            for method, scalar in methods.items():
                key = f"test.test_subprocess.{cls}.{method}"
                if args.case is not None and key != args.case:
                    continue
                directory = Path(tmp, key)
                directory.mkdir()
                fixture = (PREAMBLE.replace("TYPE", scalar) if scalar is not None else
                           "import subprocess\nimport sys\n" + testcase_fixture())
                executions[key] = subprocess_method(tools, directory, source, cls, key, fixture)
        with patch.dict(os.environ, IMRE_PYTHON=sys.executable):
            native, failures, skips = run_in_fixtures(load_suite(manifest["module"]), executions)
        if failures or skips or set(native) != set(executions):
            raise AssertionError((failures, skips, set(executions) - set(native)))
    args.result.write_text(json.dumps({"opt_level": args.opt_level,
                                       "native_body_host_fixtures": native, "blocked": {}}, indent=2) + "\n")


if __name__ == "__main__":
    main()
