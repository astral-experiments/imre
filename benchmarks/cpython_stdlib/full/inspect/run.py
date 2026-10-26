"""Measure the original inspect methods and their native operations on the pinned suite."""
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
from suite_native import NativeCompileError, Toolchain, native_method, run_evidence
import cleandoc_cases

OWNER = "TestGetattrStatic"
CASE = "test.test_inspect.test_inspect.TestGetattrStatic.test_no_dict_no_slots"
FIXTURE = """import inspect

class NativeCase:
    def assertEqual(self, left, right) -> None:
        if left != right:
            raise AssertionError("values differ")
    def assertNotEqual(self, left, right) -> None:
        if left == right:
            raise AssertionError("values compare equal")
"""

# In these cases CPython returns an existing descriptor. It must not become a
# field read, trigger __get__, or quietly use a caller-provided default.
BOUNDARIES = (
    ("from inspect import getattr_static\nprint(type(getattr_static(1, 'numerator', 0)).__name__)\n",
     "getset_descriptor", ("this builtin descriptor has no native static value",)),
    ("from dataclasses import dataclass\nfrom inspect import getattr_static\n"
     "@dataclass(slots=True)\nclass Slot:\n    value: int\n"
     "print(type(getattr_static(Slot(1), 'value')).__name__)\n",
     "member_descriptor", ("needs a native static value",)),
    ("from dataclasses import dataclass\nfrom inspect import getattr_static\n"
     "@dataclass\nclass Row:\n    value: int\n    @property\n    def checked(self) -> int:\n"
     "        raise AssertionError('property executed')\n"
     "print(type(getattr_static(Row(1), 'checked')).__name__)\n",
     "property", ("needs a native static value", "this class attribute has no native static value")),
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--reference", type=Path, default=HERE / "reference-linux.json")
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--reachable-only", action="store_true")
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("run with test-enabled CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reference = json.loads(args.reference.read_text())
    source = HERE / "upstream/Lib/test/test_inspect/test_inspect.py"
    reports = []
    for level in (0, 3):
        lang = Toolchain(args.imre, args.runtime, args.clang, level, args.reachable_only)
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            fixture = HERE / "static_attributes.py"
            check_dir = root / "controls"
            check_dir.mkdir()
            control = lang.build(fixture.read_bytes(), check_dir)
            expected = subprocess.run([sys.executable, fixture], check=True, capture_output=True, timeout=60)
            result = subprocess.run([control], check=True, capture_output=True, timeout=60)
            if (result.stdout, result.stderr) != (expected.stdout, expected.stderr):
                raise AssertionError((expected, result))
            for index, (source_text, descriptor_type, diagnostics) in enumerate(BOUNDARIES):
                where = root / f"boundary-{index}"
                where.mkdir()
                expected = subprocess.run([sys.executable, "-c", source_text], check=True, capture_output=True, text=True)
                if expected.stdout != descriptor_type + "\n" or expected.stderr:
                    raise AssertionError(expected)
                try:
                    lang.build(source_text.encode(), where)
                except NativeCompileError as error:
                    if not any(diagnostic in str(error) for diagnostic in diagnostics):
                        raise
                else:
                    raise AssertionError("descriptor replaced with a field or default")
            case_dir = root / "method"
            case_dir.mkdir()
            execution = native_method(lang, case_dir, source, OWNER, CASE, FIXTURE,
                operation=("imre", "inspect.getattr_static on an absent attribute and an unbound str descriptor"))
            clean_dir = root / "cleandoc"
            clean_dir.mkdir()
            executions = {CASE: execution, cleandoc_cases.CASE: cleandoc_cases.fixture(lang, clean_dir, source)}
            report = run_evidence(load_suite(manifest["module"]), executions, reference, level)
            for row in report["records"]:
                if row["id"] in executions and row["outcome"] != "pass":
                    raise AssertionError(row)
            reports.append(report)
            print(f"-O{level}: {report['counts']}")
    write_reports(args.result, reports, reference)


if __name__ == "__main__":
    main()
