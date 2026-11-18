"""Run unchanged basicConfig methods against the actual root handler registry."""
import argparse
import ast
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
from suite_native import NativeCompileError, Toolchain, method_bytes, native_method, run_in_fixtures, testcase_fixture

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


def critical_case(toolchain, directory, original):
    """Run the complete upstream critical method and its helper in the native child."""
    case_id = PREFIX + "test_critical"
    support_source = Path(original.support.__file__).resolve()
    if support_source != Path(original.__file__).resolve().parent / "support" / "__init__.py":
        raise ValueError("logging test imported a different support module")
    data = support_source.read_bytes()
    git_blob = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
    if git_blob != "6635ec3474e12ec6a2f684a1a93b6ec08df49a37":
        raise ValueError("support source differs from pinned CPython 3.14.5")
    node, = [n for n in ast.parse(data).body if isinstance(n, ast.FunctionDef) and n.name == "patch"]
    support_patch = b"".join(data.splitlines(keepends=True)[node.lineno - 1:node.end_lineno])
    if hashlib.sha256(support_patch).hexdigest() != "5fe18673e1a7a421fd686bddb38c5283e6974d7e36b378ee2cca618498caad9a":
        raise ValueError("support.patch differs from the audited function")
    helper = method_bytes(SOURCE, "BasicConfigTest", "_test_log")
    if hashlib.sha256(helper).hexdigest() != "573f527b82b697fc517a04b9389d2d6d0e348574450a6a0244f889c93719623c":
        raise ValueError("BasicConfigTest._test_log differs from the audited helper")

    def execute(case, method):
        if original.logging.root.handlers or original.logging.root.level != case.original_logging_level:
            raise AssertionError("original BasicConfigTest did not prepare the expected root state")
        (directory / "imre_logging_critical_support.py").write_bytes(support_patch)
        setup = SETUP.replace("LEVEL", repr(case.original_logging_level))
        fixture = (PREPARE + "\nimport imre_logging_critical_support as support\n"
                   "ORIGINAL_BASIC_CONFIG = logging.basicConfig\n" + testcase_fixture() + setup + helper.decode() +
                   "\n    def doCleanups(self) -> bool:\n"
                   "        cleaned = super().doCleanups()\n"
                   "        if logging.basicConfig is not ORIGINAL_BASIC_CONFIG:\n"
                   "            raise AssertionError('native basicConfig cleanup did not restore its identity')\n"
                   "        if logging.root.level != self.original_logging_level:\n"
                   "            raise AssertionError('native root level cleanup did not restore its value')\n"
                   "        return cleaned\n")
        call = native_method(toolchain, directory, SOURCE, "BasicConfigTest", case_id, fixture,
            operation=("imre", "Native logging.critical auto-configures the root via basicConfig()"),
            cleanups=True)
        result = call(case, method)
        result["observation"]["evidence"].update(
            helper_sha256=hashlib.sha256(helper).hexdigest(),
            support_patch_sha256=hashlib.sha256(support_patch).hexdigest(),
            assertion="Unchanged native _test_log asserts the intercepted basicConfig call has no arguments",
            fixture="CPython BasicConfigTest hooks; native empty root with the observed saved level; native registered cleanups")
        return result
    return execute


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
    critical = directory / "critical"
    critical.mkdir()
    callbacks[PREFIX + "test_critical"] = critical_case(toolchain, critical, test_logging)
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
                for name in ("logging_root_stream_config.py", "logging_basic_filename.py", "logging_file_codecs.py")]
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
