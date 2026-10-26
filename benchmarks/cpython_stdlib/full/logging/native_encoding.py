"""Run the original CP1251 and stream replacement methods in their host fixtures."""
import argparse
import ast
import hashlib
import json
import re
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
SOURCE = STDLIB / "upstream/Lib/test/test_logging.py"
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files
from suite_native import NativeCompileError, Toolchain, method_bytes, native_method, run_in_fixtures, testcase_fixture
CASE = "test.test_logging.EncodingTest.test_encoding_cyrillic_unicode"
SETTING = "test.test_logging.StreamHandlerTest.test_stream_setting"
IMPORTS = "import codecs\nimport io\nimport logging\nimport unittest\n"
ROOT = '''
def configure_root() -> io.StringIO:
    stream = io.StringIO()
    root = logging.getLogger("")
    root.setLevel(logging.DEBUG)
    capture = logging.StreamHandler(stream)
    capture.setFormatter(logging.Formatter(FORMAT))
    root.addHandler(capture)
    assert root.hasHandlers()
    return stream
'''
SETUP = '''
    root_stream: io.StringIO

    def __init__(self) -> None:
        self.root_stream = configure_root()
'''
BYTES = '''
    def assertEqual(self, first: bytes, second: bytes, msg: str | None = None) -> None:
        if first != second:
            case = unittest.TestCase()
            raise AssertionError(case._formatMessage(msg, f"{first!r} != {second!r}"))
        with open(OUTPUT, "wb") as written:
            written.write(first)
'''

IDENTITY = '''
    def assertIs(self, first, second: typing.TextIO) -> None:
        if first is not second:
            raise AssertionError("different stream objects")

    def assertIsNone(self, value) -> None:
        if value is not None:
            raise AssertionError("expected unchanged stream")
'''
FIXTURE = ("Original CPython BaseTest setup and teardown; its DEBUG root, formatter "
           "and StringIO handler are recreated in the native child")


def base_fixture(case):
    root = case.root_logger
    if (not root.isEnabledFor(10) or root.handlers != [case.root_hdlr]
            or case.root_hdlr.stream is not case.stream
            or case.root_hdlr.formatter is not case.root_formatter
            or case.root_formatter._style._fmt != case.log_format):
        raise AssertionError("the original BaseTest root fixture differs from the native setup")
    return IMPORTS + ROOT.replace("FORMAT", repr(case.log_format)) + "\n"


def stock_case():
    return testcase_fixture() + SETUP.replace("self.root_stream =", "super().__init__()\n        self.root_stream =")


def encoding_case(toolchain, work):
    """Try native TestCase first, preserving the host setup in either case."""
    body = method_bytes(SOURCE, "EncodingTest", CASE.rsplit(".", 1)[1])
    tree = ast.parse(b"class EncodingTest:\n" + body)
    expected, = [n.value for n in ast.walk(tree) if isinstance(n, ast.Constant) and isinstance(n.value, bytes)]
    raw = work / "assertion.bin"
    operation = ("imre", "Native logger dispatch, StreamHandler terminator, CP1251 writer and BytesIO contents")

    def execute(case, method):
        imports = base_fixture(case)
        try:
            call = native_method(toolchain, work, SOURCE, "EncodingTest", CASE,
                                 imports + stock_case(),
                                 operation=operation)
        except NativeCompileError as exc:
            match = re.search(r"case\.py:(\d+):(\d+): Imre: (expected (?:bool \| )?int, got bytes; annotations are enforced contracts)", str(exc))
            if not match:
                raise
            line, column, diagnostic = match.groups()
            assertion = (work / "case.py").read_text().splitlines()[int(line) - 1]
            if not assertion.lstrip().startswith("self.assertEqual(") or int(column) != assertion.index("(") + 2:
                raise
            fixture = imports + "\nclass NativeCase:\n" + SETUP + BYTES.replace("OUTPUT", repr(str(raw)))
            call = native_method(toolchain, work, SOURCE, "EncodingTest", CASE, fixture, operation=operation)
            result = call(case, method)
            written = raw.read_bytes()
            if written != expected:
                raise AssertionError((written, expected))
            details = {"assertion": "Native exact-bytes equality on the two original operands",
                       "shared_assertion_blocker": diagnostic, "actual_bytes_hex": written.hex()}
        else:
            result = call(case, method)
            details = {"assertion": "Native unittest.TestCase.assertEqual"}
        result["observation"]["evidence"].update(details, expected_bytes_hex=expected.hex(),
            fixture=FIXTURE)
        return result
    return execute



def stream_setting_case(toolchain, work):
    """Exercise all three calls and the original identity checks under BaseTest."""
    operation = ("imre", "Default sys.stderr, StreamHandler replacement and unchanged-stream identity")

    def execute(case, method):
        prefix = "import sys\nimport typing\n" + base_fixture(case)
        try:
            call = native_method(toolchain, work, SOURCE, "StreamHandlerTest", SETTING,
                                 prefix + stock_case(), operation=operation)
        except NativeCompileError as exc:
            # Fall back only at this original assertion, never on a handler operation.
            lines = (work / "case.py").read_text().splitlines()
            (line, column), = [(i, s.index("self.") + 1) for i, s in enumerate(lines, 1)
                               if s.lstrip().startswith("self.assertIs(old, sys.stderr)")]
            diagnostics = ("unknown class method",
                           "generated repr cannot format resource, callable, or custom-repr fields")
            location = f"case.py:{line}:{column}: Imre: "
            diagnostic = next((msg for msg in diagnostics
                               if any(row.endswith(location + msg) for row in str(exc).splitlines())), None)
            if diagnostic is None:
                raise
            fixture = prefix + "\nclass NativeCase:\n" + SETUP + IDENTITY
            call = native_method(toolchain, work, SOURCE, "StreamHandlerTest", SETTING,
                                 fixture, operation=operation)
            details = {"assertion": "Native object identity and None checks on the original operands",
                       "shared_assertion_blocker": f"unittest.TestCase.assertIs: {diagnostic}"}
        else:
            details = {"assertion": "Native unittest.TestCase identity assertions"}
        result = call(case, method)
        result["observation"]["evidence"].update(details, fixture=FIXTURE)
        return result
    return execute


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result", "cpython-source"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("run with the pinned CPython 3.14.5 test interpreter")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(args.cpython_source, manifest["files"] | manifest["reference_files"])
    from test import test_logging
    if Path(test_logging.__file__).read_bytes() != SOURCE.read_bytes():
        raise ValueError("loaded test_logging differs from the verified source")
    fixture = HERE.parents[3] / "tests/fixtures/files/logging_stream_delivery.py"
    expected = subprocess.run([sys.executable, "-I", "-B", fixture], capture_output=True, check=True)
    if expected.stderr or b"missing newline" not in expected.stdout.splitlines():
        raise AssertionError((expected.stdout, expected.stderr))
    report = {"cpython_commit": manifest["commit"], "denominator": 272, "levels": {}}
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        suite = load_suite(manifest["module"])
        assert len(list(cases(suite))) == report["denominator"]
        with tempfile.TemporaryDirectory() as folder:
            work = Path(folder)
            binary = tools.build(fixture.read_bytes(), work)
            control = subprocess.run([binary], capture_output=True, timeout=60, check=True)
            if (control.stdout, control.stderr) != (expected.stdout, expected.stderr):
                raise AssertionError((level, control, expected))
            cases_run = {CASE: encoding_case(tools, work), SETTING: stream_setting_case(tools, work)}
            results, failures, skips = run_in_fixtures(suite, cases_run)
            if failures or skips or set(results) != set(cases_run) or any(len(v) != 1 for v in results.values()):
                raise AssertionError((failures, skips, results))
            report["levels"][str(level)] = {"test_id": CASE, "observed": results[CASE][0]["observation"],
                "stream_setting": results[SETTING][0]["observation"],
                "control_stdout_sha256": hashlib.sha256(control.stdout).hexdigest(),
                "control_observations": len(control.stdout.splitlines())}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print("Both original StreamHandler methods passed at O0 and O3 under their CPython fixtures.")


if __name__ == "__main__":
    main()
