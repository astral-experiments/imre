"""Execute complete checked-output methods with the original module and case fixtures."""

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

METHODS = {
    "test_check_output_stderr": (
        "bytes",
        "merge child stderr into native captured stdout",
    ),
    "test_check_output_input_arg": (
        "bytes",
        "send bytes down child stdin and capture child stdout",
    ),
    "test_check_output_input_none": (
        "bytes",
        "supply EOF for explicit None and capture bytes",
    ),
    "test_check_output_input_none_text": (
        "str",
        "supply EOF for explicit None and capture text",
    ),
    "test_check_output_input_none_universal_newlines": (
        "str",
        "supply EOF for explicit None with universal newlines",
    ),
    "test_check_output_input_none_encoding_errors": (
        "str",
        "supply EOF and capture UTF-8 output with the ignore policy",
    ),
}
# Static native assertion adapters; the assertions and the full upstream method run in the binary.
PREAMBLE = """import subprocess
import sys
import unittest

class NativeCase(unittest.TestCase):
    def contains(self, member: TYPE, container: TYPE) -> bool:
        for start in range(len(container) - len(member) + 1):
            if container[start:start + len(member)] == member:
                return True
        return False

    def assertIn(self, member: TYPE, container: TYPE) -> None:
        self.assertTrue(self.contains(member, container))

    def assertNotIn(self, member: TYPE, container: TYPE) -> None:
        self.assertFalse(self.contains(member, container))
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    cls = "ProcessTestCase"
    source = HERE / "upstream/Lib/test/test_subprocess.py"
    with tempfile.TemporaryDirectory() as scratch:
        selected = {}
        for method, (scalar, operation) in METHODS.items():
            key = f"{manifest['module']}.{cls}.{method}"
            directory = Path(scratch, method)
            directory.mkdir()
            execute = native_method(
                tools,
                directory,
                source,
                cls,
                key,
                PREAMBLE.replace("TYPE", scalar),
                operation=("imre", operation),
            )

            def run(case, original, execute=execute):
                # If check_output silently inherits stdin for input=None, the
                # original child prints XX and the native assertion must fail.
                with tempfile.TemporaryFile() as inherited:
                    inherited.write(b"sentinel inherited input")
                    inherited.seek(0)
                    saved = os.dup(0)
                    try:
                        os.dup2(inherited.fileno(), 0)
                        return execute(case, original)
                    finally:
                        os.dup2(saved, 0)
                        os.close(saved)

            selected[key] = run
        with patch.dict(os.environ, IMRE_PYTHON=sys.executable):
            rows, failures, skips = run_in_fixtures(
                load_suite(manifest["module"]), selected
            )
        if failures or skips or set(rows) != set(selected):
            raise AssertionError((failures, skips, set(selected) - set(rows)))
    args.result.write_text(
        json.dumps(
            {"opt_level": args.opt_level, "native_body_host_fixture": rows}, indent=2
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
