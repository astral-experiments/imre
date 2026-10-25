"""Run original size-rotation routines on their real fixture filenames."""

import argparse
import json
from pathlib import Path
import sys
import tempfile

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures

SOURCE = STDLIB / "upstream/Lib/test/test_logging.py"
OWNER = "RotatingFileHandlerTest"
PREFIX = "test.test_logging." + OWNER + "."

FIXTURE = """import logging.handlers
import os
import unittest

class NativeCase(unittest.TestCase):
    fn: str
    message_num: int
    rmfiles: list[str]

    def __init__(self) -> None:
        super().__init__()
        self.fn = FILENAME
        self.message_num = MESSAGE_NUM
        self.rmfiles = []

    def assertIs(self, actual: bool, expected: bool) -> None:
        if actual != expected:
            raise AssertionError(f"{actual!r} is not {expected!r}")
"""


def run_case(toolchain, directory, case, original):
    fixture = FIXTURE.replace("FILENAME", repr(case.fn)).replace("MESSAGE_NUM", str(case.message_num))
    helpers = [("BaseTest", "next_message"), ("BaseFileTest", "next_rec"), ("BaseFileTest", "assertLogFile")]
    if case._testMethodName == "test_max_bytes_delay":
        helpers.append((OWNER, "test_max_bytes"))
    for owner, name in helpers:
        fixture += "\n" + method_bytes(SOURCE, owner, name).decode()
    path = Path(case.fn)
    # The original instance lives in the child. Pass its newly created backups to
    # the host's original tearDown; BaseFileTest already removes the active file.
    before = set(path.parent.glob(path.name + ".*"))
    try:
        return native_method(toolchain, directory, SOURCE, OWNER, case.id(), fixture,
                             operation=("imre", "Native file handler, descriptor position, rotation and on-disk assertions"))(case, original)
    finally:
        case.rmfiles.extend(str(p) for p in sorted(set(path.parent.glob(path.name + ".*")) - before))


def rotation_cases(toolchain, directory, names):
    def execute(case, original):
        return run_case(toolchain, directory, case, original)
    return {PREFIX + name: execute for name in names}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--case", action="append", dest="methods", required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("use a test-enabled CPython 3.14.5 with assertions enabled")
    import test
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], manifest["files"] | manifest["reference_files"])
    toolchain = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    suite = load_suite(manifest["module"])
    denominator = len(list(cases(suite)))
    with tempfile.TemporaryDirectory() as tmp:
        callbacks = rotation_cases(toolchain, Path(tmp), args.methods)
        observed, errors, skips = run_in_fixtures(suite, callbacks)
    args.result.write_text(json.dumps({"denominator": denominator, "opt_level": args.opt_level,
                                       "evidence": observed, "errors": errors, "skips": skips}, indent=2) + "\n")
    print(f"-O{args.opt_level}: {len(observed)} native routines, {len(errors)} errors, {len(skips)} skips")
    return int(bool(errors or skips or set(observed) != set(callbacks)))


if __name__ == "__main__":
    sys.exit(main())
