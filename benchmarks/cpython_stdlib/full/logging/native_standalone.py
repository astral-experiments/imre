"""Run the original handler-removal loop on a directly constructed Logger."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path[:0] = [str(HERE), str(STDLIB)]
from native_encoding import SOURCE, base_fixture
from suite_inventory import cases, load_suite, verify_files
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures, testcase_fixture


CASE = "test.test_logging.LoggerTest.test_has_handlers"
LOGGER = """
def configure_standalone(recording: RecordingHandler) -> logging.Logger:
    logger = logging.Logger(name="blah")
    logger.addHandler(recording)
    return logger
"""
SETUP = """
    recording: RecordingHandler
    logger: logging.Logger
    root_stream: io.StringIO

    def __init__(self) -> None:
        super().__init__()
        self.root_stream = configure_root()
        self.recording = RecordingHandler()
        self.logger = configure_standalone(self.recording)
"""


def handler_case(toolchain, work):
    handle = method_bytes(SOURCE, "RecordingHandler", "handle").decode()
    recording = """
class RecordingHandler(logging.NullHandler):
    records: list[logging.LogRecord]

    def __init__(self) -> None:
        super().__init__()
        self.records = []

""" + handle

    def execute(case, method):
        if (case.logger.name != "blah" or case.logger.parent is not None
                or case.logger.level != 0 or case.logger.handlers != [case.recording]
                or type(case.recording).__name__ != "RecordingHandler" or case.recording.records):
            raise AssertionError("the original standalone logger fixture changed")
        fixture = base_fixture(case) + recording + "\n" + LOGGER + "\n" + testcase_fixture() + SETUP
        run = native_method(toolchain, work, SOURCE, "LoggerTest", CASE, fixture,
                            operation=("imre", "Find and remove the standalone logger's live handlers"))
        result = run(case, method)
        result["observation"]["evidence"]["fixture"] = (
            "Original CPython LoggerTest/BaseTest setup, teardown and cleanup; the child "
            "recreates the real root stream, separate Logger('blah') and its RecordingHandler "
            "with the unchanged handle method")
        return result

    return execute


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("use the pinned CPython 3.14.5 test interpreter")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(args.cpython_source, manifest["files"] | manifest["reference_files"])
    sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    from test import test_logging
    if Path(test_logging.__file__).read_bytes() != SOURCE.read_bytes():
        raise ValueError("loaded test_logging differs from the verified source")
    control = HERE / "standalone_handlers.py"
    expected = subprocess.run([sys.executable, "-I", "-B", control], capture_output=True, check=True)
    report = {"cpython_commit": manifest["commit"], "denominator": 272, "test_id": CASE, "levels": {}}
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        suite = load_suite(manifest["module"])
        assert len(list(cases(suite))) == report["denominator"]
        with tempfile.TemporaryDirectory() as folder:
            work = Path(folder)
            actual = subprocess.run([tools.build(control.read_bytes(), work)], capture_output=True,
                                    timeout=60, check=True)
            if (actual.stdout, actual.stderr) != (expected.stdout, expected.stderr):
                raise AssertionError((level, actual, expected))
            results, failures, skips = run_in_fixtures(suite, {CASE: handler_case(tools, work)})
            if failures or skips or set(results) != {CASE} or len(results[CASE]) != 1:
                raise AssertionError((failures, skips, results))
            observed = results[CASE][0]["observation"]
            assert observed["roles"] == dict(body="imre", assertions="imre", operation="imre", fixture="mixed")
            report["levels"][str(level)] = {
                "observed": observed, "control_stdout_sha256": hashlib.sha256(actual.stdout).hexdigest(),
                "control_stderr_sha256": hashlib.sha256(actual.stderr).hexdigest(),
            }
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print("The original detached logger handler-removal test passed at O0 and O3.")


if __name__ == "__main__":
    main()
