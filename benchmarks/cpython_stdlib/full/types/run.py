"""Run unchanged SimpleNamespace methods and keep every test_types reference outcome."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import write_reports
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, native_method, run_evidence, testcase_fixture

OWNER = "SimpleNamespaceTests"
SOURCE = HERE / "upstream/Lib/test/test_types.py"
EQUALITY = """class NativeCase:
    def assertEqual(self, left, right) -> None:
        if left != right:
            raise AssertionError("values differ")
    def assertNotEqual(self, left, right) -> None:
        if left == right:
            raise AssertionError("values compare equal")
"""
MAP_EQUALITY = """class NativeCase:
    def assertEqual(self, left: dict[str, int], right: dict[str, int]) -> None:
        assert len(left) == len(right)
        for key in right:
            assert key in left and left[key] == right[key]
"""
CASES = {
    "test_attrget": (testcase_fixture(), "Imre's unittest.TestCase.assertEqual and assertRaises", "read existing fields and raise AttributeError on a miss"),
    "test_equal": (EQUALITY, "native equality and inequality assertion adapter", "compare independent SimpleNamespace dictionaries after mutation"),
    "test_unbound": (MAP_EQUALITY, "native typed assertion comparing every dict key and value", "return live dictionaries from temporary SimpleNamespace instances"),
}
BOUNDARIES = (
    ("from types import SimpleNamespace\nns = SimpleNamespace(i=1, text='a')\nprint(ns.text)\n",
     "a\n", "expected int, got str"),
    ("from types import SimpleNamespace\nns = SimpleNamespace(x=1)\nns.x = True\nprint(ns.x)\n",
     "True\n", "expected int, got bool"),
    ("from types import SimpleNamespace\nns = SimpleNamespace([('letter', 17)])\nprint(ns.letter)\n",
     "17\n", "requires an exact string-keyed dictionary"),
    ("from types import SimpleNamespace\nv = float('nan')\nprint(SimpleNamespace(x=v) == SimpleNamespace(x=v))\n",
     "True\n", "cannot preserve Python identity, equality, and representation"),
)


def controls(lang, root):
    fixture = HERE / "namespaces.py"
    directory = root / "control"
    directory.mkdir()
    expected = subprocess.run([sys.executable, fixture], capture_output=True, check=True, timeout=60)
    binary = lang.build(fixture.read_bytes(), directory)
    actual = subprocess.run([binary], capture_output=True, check=True, timeout=60)
    if (actual.stdout, actual.stderr) != (expected.stdout, expected.stderr):
        raise AssertionError((actual, expected))
    for index, (source, output, diagnostic) in enumerate(BOUNDARIES):
        directory = root / f"boundary-{index}"
        directory.mkdir()
        oracle = subprocess.run([sys.executable, "-c", source], check=True, capture_output=True, text=True)
        if (oracle.stdout, oracle.stderr) != (output, ""):
            raise AssertionError(oracle)
        try:
            lang.build(source.encode(), directory)
        except NativeCompileError as error:
            if diagnostic not in str(error):
                raise
        else:
            raise AssertionError(f"admitted unsupported input: {source}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("imre", "runtime", "clang", "result"):
        parser.add_argument(f"--{option}", type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("run with test-enabled CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reference = json.loads((HERE / "reference-linux.json").read_text())
    reports = []
    for level in (0, 3):
        lang = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level)
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            controls(lang, root)
            executions = {}
            for name, (fixture, assertions, operation) in CASES.items():
                directory = root / name
                directory.mkdir()
                case_id = f"{manifest['module']}.{OWNER}.{name}"
                executions[case_id] = native_method(
                    lang, directory, SOURCE, OWNER, case_id, "import types\n" + fixture,
                    operation=("imre", f"{operation}; assertions: {assertions}"))
            fixture = "test.test_types.UnionTests"
            skip = {(fixture + ".test_or_type_operator_reference_cycle", 1, level): {
                "kind": "upstream-skip", "fixture_variant": fixture, "platform": "linux",
                "configuration": "CPython 3.14.5 release build without Py_REF_DEBUG",
                "condition": "not hasattr(sys, 'gettotalrefcount')",
                "reason": "Cannot get total reference count.",
                "source": "Lib/test/test_types.py:1135",
            }}
            report = run_evidence(load_suite(manifest["module"]), executions, reference, level, exceptions=skip)
            for record in report["records"]:
                if record["id"] in executions and record["outcome"] != "pass":
                    raise AssertionError(record)
            print(f"-O{level}: {report['counts']}")
            reports.append(report)
    write_reports(args.result, reports, reference)


if __name__ == "__main__":
    main()
