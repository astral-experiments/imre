"""Compile unchanged CPython JSON test methods, their JSON calls and assertions."""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference import BENCH, COMMIT, cases, routine, verify

# Signatures specialize only the fixture boundary, never the upstream method.
# The expected count includes every iteration of the Unicode escape sweep.
GROUPS = {
    ("test_unicode", "TestUnicode", "str", "str", "str"): {
        "test_encoding3": 1, "test_encoding5": 1, "test_encoding7": 1,
        "test_ascii_non_printable_encode": 2, "test_big_unicode_encode": 2,
        "test_big_unicode_decode": 2, "test_unicode_decode": 0xd7ff,
    },
    ("test_unicode", "TestUnicode", "list[str]", "str", "str"): {
        "test_encoding4": 1, "test_encoding6": 1,
    },
    ("test_decode", "TestDecode", "int", "bytes", "int"): {"test_bytes": 1},
    ("test_dump", "TestDump", "dict[str, int]", "str", "str"): {"test_dumps": 1},
}


def source_for(module, owner, expected, encode, input_type, result_type):
    path = BENCH / f"upstream/Lib/test/test_json/{module}.py"
    raw = path.read_text()
    lines = raw.splitlines(keepends=True)
    cls = next(node for node in ast.parse(raw).body
               if isinstance(node, ast.ClassDef) and node.name == owner)
    methods = {}
    for node in cls.body:
        if isinstance(node, ast.FunctionDef) and node.name in expected:
            if node.decorator_list:
                raise ValueError("this fixture does not implement decorators")
            methods[node.name] = "".join(lines[node.lineno - 1:node.end_lineno])
    if methods.keys() != expected.keys():
        raise ValueError(f"incomplete method selection in {path}")
    fixture = f"""import json

class Case:
    checks: int
    def __init__(self) -> None:
        self.checks = 0
    def assertEqual(self, left: {result_type}, right: {result_type}) -> None:
        if left != right:
            raise AssertionError("native operands differ")
        self.checks += 1
    def loads(self, value: {input_type}) -> {result_type}:
        return json.loads(value)
    def dumps(self, value: {encode}, ensure_ascii: bool = True) -> str:
        return json.dumps(value, ensure_ascii=ensure_ascii)

"""
    entry = "".join(f"case = Case()\ncase.{name}()\nprint(case.checks)\n" for name in methods)
    return fixture + "\n".join(methods.values()) + "\n" + entry, methods


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    verify()
    fixture_ids = {}
    for case in cases():
        fixture_ids.setdefault(routine(case), set()).add(case.id())
    report = {"cpython_commit": COMMIT, "execution": "native complete method and assertions",
              "routines": {}}
    ok = True
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        source, exe = root / "case.py", root / "case"
        for (module, owner, encode, input_type, result_type), expected in GROUPS.items():
            generated, methods = source_for(module, owner, expected, encode, input_type, result_type)
            source.write_text(generated)
            labels = {}
            for name, body in methods.items():
                key = f"test.test_json.{module}.{owner}.{name}"
                labels[name] = report["routines"][key] = {
                    "fixture_ids": sorted(fixture_ids[key]),
                    "method_sha256": hashlib.sha256(body.encode()).hexdigest(), "levels": {},
                }
            for level in ("0", "3"):
                compiled = subprocess.run([args.imre.resolve(), "build", source, "-o", exe,
                    "--opt-level", level, "--clang", args.clang.resolve(),
                    "--runtime", args.runtime.resolve(), "--no-cache"], cwd=root,
                    capture_output=True, text=True, timeout=180)
                if compiled.returncode:
                    ok = False
                    print(compiled.stderr, file=sys.stderr)
                    for row in labels.values():
                        row["levels"][level] = {"compile_error": compiled.stderr.replace(str(root), "<tmp>")}
                    continue
                run = subprocess.run([exe], cwd=root, capture_output=True, text=True, timeout=60)
                counts = run.stdout.splitlines()
                for i, name in enumerate(methods):
                    if not run.returncode and not run.stderr and len(counts) == len(methods) and counts[i] == str(expected[name]):
                        labels[name]["levels"][level] = {"pass": True, "assertions": expected[name]}
                    else:
                        ok = False
                        labels[name]["levels"][level] = {"failure": [run.returncode, counts, run.stderr]}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(not ok)


if __name__ == "__main__":
    sys.exit(main())
