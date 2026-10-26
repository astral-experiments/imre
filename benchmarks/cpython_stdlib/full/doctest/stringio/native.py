"""Verify native StringIO streams and complete upstream CStringIOTest methods."""

import argparse
from difflib import unified_diff
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parents[2]), str(HERE)]
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures, testcase_fixture
from stream_fixture import CASES as HOST_CASES, executor as host_executor

SOURCE = "Lib/test/test_memoryio.py"
BLOB = "482b183da23ffa259664e52592021d5c3c12d34d"
CASES = {
    **dict.fromkeys(("testInit", "testRead", "testReadNoArgs", "testTell"), "MemorySeekTestMixin"),
    "test_flush": "MemoryTestMixin",
    "test_issue5265": "TextIOTestMixin",
    "test_textio_properties": "TextIOTestMixin",
    "test_widechar": "CStringIOTest",
}


def fixture(method):
    if method == "test_flush":
        # The bundled equality currently accepts scalar or list operands, not None.
        case = """class NativeCase:
    def assertEqual(self, first: None, second: None) -> None:
        if first != second:
            raise AssertionError("unequal stream results")
"""
    else:
        case = testcase_fixture() + """
    EOF: str

    def __init__(self) -> None:
        super().__init__()
        self.EOF = ""
"""
    return "from io import StringIO\n" + case + """

    def buftype(self, text: str) -> str:
        return str(text)

    def ioclass(self, initial_value: str | None = "", newline: str | None = "\\n") -> StringIO:
        return StringIO(initial_value, newline)
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise ValueError("the fixture and differential oracle require CPython 3.14.5")
    manifest = json.loads((HERE.parent / "UPSTREAM.json").read_bytes())
    verify_files(HERE.parent / "upstream", manifest["files"])
    verify_files(args.cpython_source, {SOURCE: BLOB})
    # The staged original test package supplies the class and its real fixtures.
    sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    verify_interpreter_tests({"files": {SOURCE: BLOB}})
    source = args.cpython_source.resolve() / SOURCE
    discovered = {c.id() for c in cases(load_suite("test.test_memoryio"))
                  if c.id().startswith("test.test_memoryio.CStringIOTest.")}
    selected = {"test.test_memoryio.CStringIOTest." + name for name in (*CASES, *HOST_CASES)}
    if len(discovered) != 39 or not selected <= discovered:
        raise ValueError("the selected prerequisite methods do not match the original class")
    stream = (HERE / "streams.py").read_bytes()
    reference = subprocess.run([sys.executable, "-B", HERE / "streams.py"],
                               capture_output=True, timeout=60, check=True)
    report = {"cpython": "5607950ef232dad16d75c0cf53101d9649d89115",
              "source": SOURCE, "blob": BLOB, "observations": {}}
    for level in (0, 3):
        toolchain = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = toolchain.build(stream, root)
            actual = subprocess.run([binary], capture_output=True, timeout=60, check=True)
            if (actual.stdout, actual.stderr) != (reference.stdout, reference.stderr):
                diff = "".join(unified_diff(reference.stdout.decode().splitlines(keepends=True),
                                            actual.stdout.decode().splitlines(keepends=True),
                                            fromfile="CPython", tofile=f"Imre O{level}"))
                raise AssertionError(f"O{level} stream transcript differs from CPython:\n{diff}\n"
                                     f"stderr: {reference.stderr!r} / {actual.stderr!r}")
            executions = {}
            for method, owner in CASES.items():
                case = f"test.test_memoryio.CStringIOTest.{method}"
                directory = root / method
                directory.mkdir()
                executions[case] = native_method(toolchain, directory, source, owner, case,
                                                 fixture(method),
                                                 operation=("imre", "native io.StringIO"))
            driver = root / "stream-driver"
            driver.mkdir()
            stream_binary = toolchain.build((HERE / "driver.py").read_bytes(), driver)
            for method, owner in HOST_CASES.items():
                case = f"test.test_memoryio.CStringIOTest.{method}"
                executions[case] = host_executor(stream_binary, source, owner, method, level)
            evidence, failures, skips = run_in_fixtures(load_suite("test.test_memoryio"), executions)
            if failures or skips or evidence.keys() != executions.keys():
                raise AssertionError((failures, skips, sorted(executions.keys() - evidence.keys())))
            report["observations"][f"O{level}"] = {k: v[0]["observation"] for k, v in evidence.items()}
            print(f"O{level}: stream transcript matches; {len(CASES)} native bodies under CPython fixtures; "
                  f"{len(HOST_CASES)} original host bodies drive native streams")
    args.result.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
