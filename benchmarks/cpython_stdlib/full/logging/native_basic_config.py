"""Run unchanged basicConfig methods against the actual root handler registry."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
SOURCE = STDLIB / "upstream/Lib/test/test_logging.py"
sys.path[:0] = [str(HERE), str(STDLIB)]
from native_basic_config_options import option_cases
from suite_inventory import cases, load_suite, verify_files
from suite_native import NativeCompileError, Toolchain, native_method, run_in_fixtures, testcase_fixture

PREFIX = "test.test_logging.BasicConfigTest."
METHODS = {
    "test_datefmt": "Read the date format of the formatter attached to the root handler",
    "test_force": "Close and replace the live root handler; verify both streams and the new threshold",
}
PREPARE = """import io
import logging
import sys
import unittest

def prepare(level: int) -> int:
    root = logging.getLogger("")
    assert not root.handlers
    root.setLevel(level)
    return root.level
"""
SETUP = """
    original_logging_level: int

    def __init__(self) -> None:
        super().__init__()
        self.original_logging_level = prepare(LEVEL)
"""


DATE_ASSERTION = """
class NativeCase:
    def assertEqual(self, first: str | None, second: str) -> None:
        if first is None:
            raise AssertionError("expected a date format")
        if first != second:
            raise AssertionError("different date formats")
"""


def config_cases(toolchain, directory):
    from test import test_logging

    def configure(method, description):
        case_id = PREFIX + method
        work = directory / method
        work.mkdir()

        def execute(case, original):
            if test_logging.logging.root.handlers or test_logging.logging.root.level != case.original_logging_level:
                raise AssertionError("original BasicConfigTest did not prepare an empty root with the saved level")
            setup = SETUP.replace("LEVEL", repr(case.original_logging_level))
            operation = ("imre", description)
            try:
                call = native_method(toolchain, work, SOURCE, "BasicConfigTest", case_id,
                                     PREPARE + "\n" + testcase_fixture() + setup, operation=operation)
            except NativeCompileError as exc:
                diagnostic = "expected bool | int, got str | None; annotations are enforced contracts"
                if method != "test_datefmt":
                    raise
                lines = (work / "case.py").read_text().splitlines()
                (line, column), = [(i, text.index("formatter.datefmt") + 1) for i, text in enumerate(lines, 1)
                                   if text.strip() == "self.assertEqual(formatter.datefmt, 'bar')"]
                if f"case.py:{line}:{column}: Imre: {diagnostic}" not in str(exc):
                    raise
                fixture = PREPARE + DATE_ASSERTION + setup.replace("        super().__init__()\n", "")
                call = native_method(toolchain, work, SOURCE, "BasicConfigTest", case_id, fixture, operation=operation)
                details = {"assertion": "Native optional string equality on the two original operands",
                           "shared_assertion_blocker": diagnostic}
            else:
                details = {"assertion": "Native unittest.TestCase on the original operands"}
            result = call(case, original)
            result["observation"]["evidence"].update(
                details,
                fixture="CPython BasicConfigTest setup, teardown and cleanup; native child starts with the observed empty root and saved level")
            return result
        return execute

    callbacks = {PREFIX + method: configure(method, description) for method, description in METHODS.items()}
    options = directory / "file-options"
    options.mkdir()
    callbacks.update(option_cases(toolchain, options, SOURCE, PREPARE, SETUP))
    return callbacks


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result", "cpython-source"):
        parser.add_argument("--" + arg, required=True, type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("run with the pinned CPython 3.14.5 test interpreter")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(args.cpython_source, manifest["files"] | manifest["reference_files"])
    from test import test_logging
    if Path(test_logging.__file__).read_bytes() != SOURCE.read_bytes():
        raise ValueError("loaded test_logging differs from the verified source")
    fixtures = [HERE.parents[3] / "tests/fixtures/files" / name
                for name in ("logging_root_stream_config.py", "logging_basic_filename.py")]
    expected = {}
    for source in fixtures:
        with tempfile.TemporaryDirectory() as tmp:
            oracle = subprocess.run([sys.executable, "-I", "-B", source], cwd=tmp,
                                    capture_output=True, timeout=60, check=True)
        if oracle.stderr:
            raise AssertionError(oracle.stderr)
        expected[source.name] = oracle.stdout
    report = {"cpython_commit": manifest["commit"], "denominator": 272, "levels": {}}
    for level in (0, 3):
        suite = load_suite(manifest["module"])
        if len(list(cases(suite))) != report["denominator"]:
            raise ValueError("logging discovery changed")
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            controls = {}
            for source in fixtures:
                directory = work / source.stem
                directory.mkdir()
                binary = tools.build(source.read_bytes(), directory)
                control = subprocess.run([binary], cwd=directory, capture_output=True, timeout=60, check=True)
                if control.stdout != expected[source.name] or control.stderr:
                    raise AssertionError((level, source.name, control, expected[source.name]))
                controls[source.name] = {"observations": len(control.stdout.splitlines()),
                                         "stdout_sha256": hashlib.sha256(control.stdout).hexdigest()}
            executions = config_cases(tools, work)
            records, failures, skips = run_in_fixtures(suite, executions)
            if failures or skips or set(records) != set(executions) or any(len(v) != 1 for v in records.values()):
                raise AssertionError((failures, skips, records))
            report["levels"][str(level)] = {"observed": {k: v[0].get("observation", v[0]) for k, v in records.items()},
                "controls": controls}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print("Original root configuration methods passed at O0 and O3 under their CPython fixtures.")


if __name__ == "__main__":
    main()
