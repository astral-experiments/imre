"""Run the two original string core methods with a counted, native str assertion."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, roles, validate_pair
from suite_inventory import verify_files

METHODS = {"test_attrs", "test_capwords"}
ADAPTER = '''import string

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
    return subprocess.run([exe], check=True, capture_output=True, cwd=work, timeout=60).stdout


def methods(upstream):
    src = upstream.read_bytes()
    lines = src.splitlines(keepends=True)
    cls = next(node for node in ast.parse(src).body if isinstance(node, ast.ClassDef) and node.name == "ModuleTest")
    for node in cls.body:
        if not isinstance(node, ast.FunctionDef) or node.name not in METHODS:
            continue
        assert not node.decorator_list
        yield node.name, b"".join(lines[node.lineno - 1:node.end_lineno])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--opt-levels", nargs="+", type=int, choices=(0, 3), default=(0, 3))
    parser.add_argument("--sanitize", action="store_true")
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    ref = json.loads((HERE / "reference-linux.json").read_text())
    source = HERE / "upstream/Lib/test/test_string/test_string.py"
    original = dict(methods(source))
    assert original.keys() == METHODS and all(ref["reference"][f"{manifest['module']}.test_string.ModuleTest.{m}"] == "pass" for m in original)
    extra = HERE / "core_differential.py"
    expected = subprocess.check_output([args.python, "-E", extra])
    reports = []
    for level in args.opt_levels:
        observations = {}
        with tempfile.TemporaryDirectory() as d:
            work = Path(d)
            for name, method in original.items():
                # The method and its expected operands are byte-for-byte upstream.
                entry = f'''\ncase = Case()\ncase.{name}()\nprint(case.checks)
try:
    case.assertEqual("positive", "negative")
except AssertionError as exc:
    print(str(exc))
'''
                got = compile_run(args, level, ADAPTER.encode() + method + entry.encode(), work)
                if got != b"9\nunequal strings\n":
                    raise AssertionError((name, got))
                key = f"{manifest['module']}.test_string.ModuleTest.{name}"
                observations[(key, 1)] = {"outcome": "pass", "roles": roles(
                    body="imre", assertions="imre", operation="imre", fixture="imre"),
                    "evidence": {"operation": f"compiled {name} against the bundled string module",
                                 "fixture": "byte-identical method in a native counted str equality adapter",
                                 "method_sha256": hashlib.sha256(method).hexdigest(),
                                 "source_assertions": 9, "negative_control": "unequal strings"}}
            if compile_run(args, level, extra.read_bytes(), work) != expected:
                raise AssertionError("supplemental Unicode and separator comparison failed")
        variants = [(key, "original method, native str assertEqual" if (key, 1) in observations
                     else "upstream discovery") for key in ref["reference"]]
        reports.append(build_report(ref, level, variants, observations))
        print(f"O{level}: 2/54 original methods; 18 assertions, two negative controls, {len(expected.splitlines())} supplemental output lines matched")
    if args.evidence:
        validate_pair(reports, ref)
        args.evidence.write_text(json.dumps(reports, indent=2) + "\n")


if __name__ == "__main__":
    main()
