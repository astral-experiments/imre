"""Exercise the original child-logger methods on the process-local logging registry."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
SOURCE = STDLIB / "upstream/Lib/test/test_logging.py"
sys.path[:0] = [str(HERE), str(STDLIB)]
from suite_inventory import cases, load_suite, verify_files
from suite_native import NativeCompileError, Toolchain, native_method, run_in_fixtures
from native_cache import cache_assertion_blocker, check_cache_values, host_cache_case

FIXTURE = """import logging

class NativeCase:
    def assertEqual(self, first: set[logging.Logger], second: set[logging.Logger]) -> None:
        if first != second:
            raise AssertionError("logger sets differ")

    def assertNotIn(self, logger: logging.Logger, members: set[logging.Logger]) -> None:
        if logger in members:
            raise AssertionError("unexpected logger")
"""


def hierarchy_cases(toolchain, work):
    case_id = "test.test_logging.ChildLoggerTest.test_get_children"
    children = work / "children"
    propagation = work / "propagation"
    children.mkdir()
    propagation.mkdir()
    stopped_id = "test.test_logging.LoggerTest.test_has_handlers_no_propagate"
    fixture = '''import logging
import unittest

def configure_root() -> bool:
    logging.basicConfig(level=logging.DEBUG, format="%(name)s -> %(levelname)s: %(message)s")
    return logging.getLogger().hasHandlers()

class NativeCase(unittest.TestCase):
    def __init__(self) -> None:
        super().__init__()
        assert configure_root()
'''
    execute = native_method(toolchain, propagation, SOURCE, "LoggerTest", stopped_id,
        fixture, operation=("imre", "Stop handler lookup at the child before reaching a configured root"))

    def stopped(case, method):
        if case.root_hdlr not in case.root_logger.handlers or case.logger.parent is not None:
            raise AssertionError("expected the original root handler and standalone Logger('blah')")
        result = execute(case, method)
        result["observation"]["evidence"]["fixture"] = (
            "Original LoggerTest/BaseTest hooks on CPython; native basicConfig recreates the "
            "observed root handler presence used by this method")
        return result

    caching = work / "caching"
    caching.mkdir()
    check_cache_values(toolchain, caching)
    caching_id = "test.test_logging.LoggerTest.test_caching"
    cache_fixture = """import logging
import unittest

def configured_root() -> logging.Logger:
    logging.basicConfig(level=logging.DEBUG, format="%(name)s -> %(levelname)s: %(message)s")
    return logging.getLogger()

class NativeCase(unittest.TestCase):
    root_logger: logging.Logger

    def __init__(self) -> None:
        super().__init__()
        self.root_logger = configured_root()
"""
    try:
        execute_cache = native_method(toolchain, caching, SOURCE, "LoggerTest", caching_id, cache_fixture,
            operation=("imre", "Populate logger caches and invalidate them on level and disable changes"))
    except NativeCompileError as exc:
        blocker = cache_assertion_blocker(exc, caching)
        if blocker is None:
            raise
        cached = host_cache_case(toolchain, caching, blocker)
    else:
        def cached(case, method):
            if case.root_logger.level != 10 or case.root_hdlr not in case.root_logger.handlers:
                raise AssertionError("expected the original LoggerTest root at DEBUG with its handler")
            result = execute_cache(case, method)
            result["observation"]["evidence"]["fixture"] = (
                "Original LoggerTest/BaseTest hooks on CPython; native basicConfig supplies an isolated root at DEBUG")
            return result

    return {
        case_id: native_method(toolchain, children, SOURCE, "ChildLoggerTest", case_id,
            FIXTURE, operation=("imre", "Enumerate direct logger children by identity, excluding missing ancestors")),
        stopped_id: stopped,
        caching_id: cached,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("requires a test-enabled CPython 3.14.5 with assertions enabled")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], manifest["files"] | manifest["reference_files"])
    suite = load_suite(manifest["module"])
    denominator = len(list(cases(suite)))
    tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        def output(cmd):
            child = subprocess.run(cmd, capture_output=True, timeout=30, check=True)
            return child.stdout, child.stderr
        for filename, commands in (("hierarchy_values.py", [()]),
                                   ("reconfigure_handlers.py", [("dict",), ("file", work / "logging.ini")])):
            sample = HERE / filename
            program = tools.build(sample.read_bytes(), work)
            for arguments in commands:
                expected = output([sys.executable, sample, *arguments])
                actual = output([program, *arguments])
                if actual != expected:
                    raise AssertionError((filename, arguments, expected, actual))
        callbacks = hierarchy_cases(tools, work)
        observed, failures, skips = run_in_fixtures(suite, callbacks)
        if failures or skips or set(observed) != set(callbacks):
            raise AssertionError((failures, skips, observed))
    args.result.write_text(json.dumps({"denominator": denominator, "opt_level": args.opt_level,
                                       "evidence": observed}, indent=2) + "\n")
    for case_id, records in observed.items():
        for record in records:
            evidence = record.get("observation", record)
            print(f"-O{args.opt_level} {case_id}: {evidence['roles']}")


if __name__ == "__main__":
    main()
