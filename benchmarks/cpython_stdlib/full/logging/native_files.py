"""Run original logging file-handler methods on their real fixture filenames."""

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
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures

SOURCE = STDLIB / "upstream/Lib/test/test_logging.py"

FILE_FIXTURE = """import logging
import os
import unittest
from typing import TextIO

class NativeCase:
    fn: str
    checks: unittest.TestCase

    def __init__(self) -> None:
        self.fn = FILENAME
        self.checks = unittest.TestCase()

    def assertIsNone(self, stream: TextIO | None) -> None:
        self.checks.assertTrue(stream is None)

    def assertIsNotNone(self, stream: TextIO | None) -> None:
        self.checks.assertFalse(stream is None)

    def assertTrue(self, value: bool) -> None:
        self.checks.assertTrue(value)

    def assertFalse(self, value: bool) -> None:
        self.checks.assertFalse(value)
"""


WRITE_FIXTURE = """import logging
import os

class NativeCase:
    fn: str
    message_num: int

    def __init__(self) -> None:
        self.fn = FILENAME
        self.message_num = MESSAGE_NUM

    def assertEqual(self, first: str, second: str) -> None:
        if first != second:
            raise AssertionError(f"{first!r} != {second!r}")
"""


def file_cases(toolchain, work):
    prefix = "test.test_logging.FileHandlerTest."
    def delay(case, method):
        result = native_method(toolchain, work, SOURCE,
                             "FileHandlerTest", case.id(), FILE_FIXTURE.replace("FILENAME", repr(case.fn)),
                             operation=("imre", "Native delayed file creation, record emission and close"))(case, method)
        result["observation"]["evidence"]["fixture"] = (
            "Original CPython FileHandlerTest hooks and filename; the native fixture checks optional "
            "stream identity and passes the four predicates to native TestCase boolean assertions")
        return result
    def write_mode(case, method):
        fixture = WRITE_FIXTURE.replace("FILENAME", repr(case.fn)).replace("MESSAGE_NUM", str(case.message_num))
        for owner, helper in (("BaseTest", "next_message"), ("BaseFileTest", "next_rec")):
            fixture += "\n" + method_bytes(SOURCE, owner, helper).decode()
        return native_method(toolchain, work, SOURCE, "FileHandlerTest", case.id(), fixture,
                             operation=("imre", "Native file writes, formatter, close and readback"))(case, method)
    return {prefix + "test_delay": delay, prefix + "test_emit_after_closing_in_write_mode": write_mode}


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
        sample = HERE / "file_values.py"
        binary = tools.build(sample.read_bytes(), work)
        def observe(cmd, dest, report):
            args = [*cmd, dest, *(["report"] if report else [])]
            child = subprocess.run(args, capture_output=True, check=True, timeout=30)
            if report:
                expected = ["--- Logging error ---", "UnsupportedOperation: not writable",
                            "Call stack:", "Message: 'π'", "Arguments: ()"]
                lines = [line.removeprefix("io.") for line in child.stderr.decode().splitlines()]
                if [line for line in lines if line in expected] != expected:
                    raise AssertionError((cmd, child.stderr))
            elif child.stderr:
                raise AssertionError((cmd, child.stderr))
            return child.stdout, dest.read_bytes()
        for report in (False, True):
            actual = observe([binary], work / f"native-{report}.log", report)
            reference = observe([sys.executable, sample], work / f"reference-{report}.log", report)
            if actual != reference:
                raise AssertionError("native file lifecycle or bytes differ from CPython")
        callbacks = file_cases(tools, work)
        observed, errors, skips = run_in_fixtures(suite, callbacks)
        if errors or skips or set(observed) != set(callbacks):
            raise AssertionError((errors, skips, observed))
    args.result.write_text(json.dumps({"denominator": denominator, "opt_level": args.opt_level,
                                       "evidence": observed}, indent=2) + "\n")
    print(f"-O{args.opt_level}: {len(observed)} native bodies and assertions; original CPython files and hooks")


if __name__ == "__main__":
    main()
