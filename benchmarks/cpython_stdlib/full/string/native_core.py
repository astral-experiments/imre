"""Run original string methods with native assertions and compare them with CPython."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from suite_evidence import build_report, roles, validate_pair
from suite_inventory import verify_files
from template_controls import controls
from template_unittest import REPAIRED_INPUT, run_method

METHODS = {
    "ModuleTest": {"test_attrs", "test_capwords"},
    "TestTemplate": {
        "test_regular_templates_with_upper_case", "test_regular_templates_with_non_letters",
        "test_escapes", "test_percents", "test_SafeTemplate", "test_unicode_values",
        "test_keyword_arguments", "test_stringification", "test_tupleargs",
    } | set(REPAIRED_INPUT),
}
ADAPTER = '''import string
from string import Template

class Case:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def assertEqual(self, first: str, second: str) -> None:
        self.checks += 1
        if first != second:
            raise AssertionError("unequal strings")

'''


def compile_run(args, level, source, work):
    path = work / "case.py"
    path.write_bytes(source)
    exe = work / "case"
    cmd = [args.imre.resolve(), "build", path, "-o", exe, "--runtime", args.runtime.resolve(),
           "--no-cache", "--clang", args.clang, "--opt-level", str(level)]
    if args.sanitize:
        cmd.append("--sanitize")
    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, cwd=work, timeout=120)
    result = subprocess.run([exe], capture_output=True, cwd=work, timeout=60)
    if result.returncode or result.stderr:
        raise AssertionError(f"native process exited {result.returncode}: {result.stderr.decode(errors='replace')}")
    return result.stdout


def methods(upstream):
    src = upstream.read_bytes()
    lines = src.splitlines(keepends=True)
    for cls in ast.parse(src).body:
        if not isinstance(cls, ast.ClassDef) or cls.name not in METHODS:
            continue
        for node in cls.body:
            if isinstance(node, ast.FunctionDef) and node.name in METHODS[cls.name]:
                assert not node.decorator_list
                yield (cls.name, node.name), b"".join(lines[node.lineno - 1:node.end_lineno])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--opt-levels", nargs="+", type=int, choices=(0, 3), default=(0, 3))
    parser.add_argument("--sanitize", action="store_true")
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--cpython-source", type=Path, help="also verify the upstream source of the logging validator")
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    ref = json.loads((HERE / "reference-linux.json").read_text())
    source = HERE / "upstream/Lib/test/test_string/test_string.py"
    original = dict(methods(source))
    owners = json.loads((HERE / "template-cases.json").read_text())
    assert set(owners["native_callable_assertions"]) == set(REPAIRED_INPUT)
    assert set(owners["native_default_pattern"]) | set(REPAIRED_INPUT) == METHODS["TestTemplate"]
    prefix = f"{manifest['module']}.test_string.TestTemplate."
    upstream_ids = {key.removeprefix(prefix) for key in ref["reference"] if key.startswith(prefix)}
    grouped = [name for group in owners.values() for name in group]
    assert len(grouped) == len(upstream_ids) == 24 and set(grouped) == upstream_ids
    assert original.keys() == {(cls, name) for cls, names in METHODS.items() for name in names}
    assert all(ref["reference"][f"{manifest['module']}.test_string.{cls}.{name}"] == "pass" for cls, name in original)
    extra = HERE / "core_differential.py"
    expected = subprocess.check_output([args.python, "-E", extra])
    reports = []
    for level in args.opt_levels:
        observations = {}
        with tempfile.TemporaryDirectory() as d:
            work = Path(d)
            assertions = 0
            for (cls, name), method in original.items():
                key = f"{manifest['module']}.test_string.{cls}.{name}"
                if cls == "TestTemplate" and name in REPAIRED_INPUT:
                    checks, failure = run_method(args, level, work, name, method, compile_run)
                    assertions += checks
                    observations[(key, 1)] = {"outcome": "pass", "roles": roles(
                        body="imre", assertions="imre", operation="imre", fixture="imre"),
                        "evidence": {"operation": f"compiled {name} against the bundled string module",
                                     "fixture": "unchanged method on native unittest.TestCase; separate repaired-input copy fails in assertRaises",
                                     "method_sha256": hashlib.sha256(method).hexdigest(),
                                     "source_assertions": checks, "negative_control": failure}}
                    continue
                # The method and its expected operands are byte-for-byte upstream.
                entry = f'''\ncase = Case()\ncase.{name}()\nprint(case.checks)
try:
    case.assertEqual("positive", "negative")
except AssertionError as exc:
    print(str(exc))
'''
                program = ADAPTER.encode() + method + entry.encode()
                expected_method = subprocess.run([args.python, "-E", "-"], input=program, check=True, capture_output=True, cwd=work).stdout
                got = compile_run(args, level, program, work)
                if got != expected_method or got.splitlines()[1:] != [b"unequal strings"]:
                    raise AssertionError((name, got, expected_method))
                checks = int(got.splitlines()[0])
                assert checks > 0
                assertions += checks
                observations[(key, 1)] = {"outcome": "pass", "roles": roles(
                    body="imre", assertions="imre", operation="imre", fixture="imre"),
                    "evidence": {"operation": f"compiled {name} against the bundled string module",
                                 "fixture": "byte-identical method in a native counted str equality adapter",
                                 "method_sha256": hashlib.sha256(method).hexdigest(),
                                 "source_assertions": checks, "negative_control": "unequal strings"}}
            if compile_run(args, level, extra.read_bytes(), work) != expected:
                raise AssertionError("supplemental Unicode and separator comparison failed")
            template_lines, logging_formats = controls(args, level, work, compile_run)
        callback_ids = {prefix + name for name in REPAIRED_INPUT}
        variants = [(key, "upstream discovery" if (key, 1) not in observations else
                     "original method, native unittest.TestCase callbacks" if key in callback_ids else
                     "original method, native str assertEqual") for key in ref["reference"]]
        reports.append(build_report(ref, level, variants, observations))
        print(f"O{level}: {len(original)}/54 original methods; {assertions} assertions, {len(original)} negative controls, {len(expected.splitlines())} core and {template_lines} Template output lines; {logging_formats} upstream logging formats matched")
    if args.evidence:
        validate_pair(reports, ref)
        args.evidence.write_text(json.dumps(reports, indent=2) + "\n")


if __name__ == "__main__":
    main()
