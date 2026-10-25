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
from suite_native import Toolchain, native_method, run_in_fixtures

# The subprocess operation, method body, and assertions all run in the binary.
# sys.executable selects a real oracle child for upstream's literal -c programs.
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
        "test_communicate_stdout": "bytes",
        "test_communicate_stderr": "bytes",
        "test_communicate_returns": "bytes",
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



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    source = HERE / "upstream/Lib/test/test_subprocess.py"
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    if sys.version_info[:3] != (3, 14, 5):
        raise RuntimeError("use the pinned CPython 3.14.5 build")
    tools = Toolchain(args.imre, args.runtime, args.clang, int(args.opt_level))
    executions, blocked = {}, {}
    with tempfile.TemporaryDirectory() as tmp:
        for cls, methods in METHODS.items():
            for method, scalar in methods.items():
                key = f"test.test_subprocess.{cls}.{method}"
                directory = Path(tmp, key)
                directory.mkdir()
                try:
                    executions[key] = native_method(tools, directory, source, cls, key,
                                                    PREAMBLE.replace("TYPE", scalar))
                except (ValueError, AssertionError, RuntimeError) as exc:
                    if method != "test_post_timeout_communicate_sends_input" or "local may be uninitialized on this path" not in str(exc):
                        raise
                    blocked[key] = "Imre does not infer that the except handler calling self.fail() never returns."
        with patch.dict(os.environ, IMRE_PYTHON=sys.executable):
            native, failures, skips = run_in_fixtures(load_suite(manifest["module"]), executions)
        if failures or skips or set(native) != set(executions):
            raise AssertionError((failures, skips, set(executions) - set(native)))
    args.result.write_text(json.dumps({"opt_level": args.opt_level,
                                       "native_body_host_fixtures": native, "blocked": blocked}, indent=2) + "\n")


if __name__ == "__main__":
    main()
