"""Compile complete, unchanged itertools methods with native assertion adapters."""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
# Each entry gives the assertEqual signature, its expected dynamic assertion count,
# and the globals imported by the original method. Test bodies come from the pin.
CASES = {
    "LengthTransparency.test_repeat": ("int", 3, ("repeat",)),
    "LengthTransparency.test_repeat_with_negative_times": ("int", 4, ("repeat",)),
    "TestBasicOps.test_repeat_with_negative_times": ("str", 4, ("repeat",)),
    "TestExamples.test_repeat": ("list[int]", 1, ("repeat",)),
    "TestExamples.test_chain": ("str", 1, ("chain",)),
    "TestExamples.test_filter": ("list[int]", 1, ()),
    "TestExamples.test_zip": ("list[tuple[str, str]]", 1, ()),
}


def sources():
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    for name, oid in manifest["files"].items():
        data = (HERE / "upstream" / name).read_bytes()
        if hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest() != oid:
            raise ValueError(f"unexpected upstream blob: {name}")
    data = (HERE / "upstream/Lib/test/test_itertools.py").read_bytes()
    lines = data.splitlines(keepends=True)
    methods = {
        f"{cls.name}.{node.name}": node
        for cls in ast.parse(data).body if isinstance(cls, ast.ClassDef)
        for node in cls.body if isinstance(node, ast.FunctionDef)
    }
    for name, (ty, checks, imports) in CASES.items():
        node = methods[name]
        if node.decorator_list:
            raise ValueError(f"guarded method needs its upstream fixture: {name}")
        method = b"".join(lines[node.lineno - 1:node.end_lineno])
        header = ("from itertools import " + ", ".join(imports) + "\n") if imports else ""
        if b"operator." in method:
            header += "import operator\n"
        equality = '        if actual != expected:\n            raise AssertionError("assertEqual")'
        if ty.startswith("list["):
            equality = """        if len(actual) != len(expected):
            raise AssertionError("unequal lengths")
        for i in range(len(actual)):
            if actual[i] != expected[i]:
                raise AssertionError("unequal elements")"""
        adapter = f"""
class Case:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def assertEqual(self, actual: {ty}, expected: {ty}) -> None:
{equality}
        self.checks += 1

"""
        entry = f"\ncase = Case()\ncase.{node.name}()\nprint(case.checks)\n"
        yield (manifest["module"] + "." + name, checks, hashlib.sha256(method).hexdigest(),
               (header + adapter).encode() + method + entry.encode())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--opt-level", type=int, required=True, choices=(0, 3))
    args = parser.parse_args()
    command = [args.imre.resolve(), "build", "case.py", "-o", "case", "--no-cache",
               "--runtime", args.runtime.resolve(), "--clang", args.clang.resolve(),
               "--opt-level", str(args.opt_level)]
    reference = json.loads((HERE / "reference-linux.json").read_text())
    results = {}
    for name, checks, digest, source in sources():
        if reference["reference"][name] != "pass":
            raise ValueError(f"expected an executed reference case: {name}")
        with tempfile.TemporaryDirectory() as directory:
            wd = Path(directory)
            (wd / "case.py").write_bytes(source)
            built = subprocess.run(command, cwd=wd, capture_output=True, text=True, timeout=120)
            result = {"method_sha256": digest, "assertions": checks}
            if built.returncode:
                result.update(status="compile_error", diagnostic=built.stderr.strip())
            else:
                ran = subprocess.run([wd / "case"], cwd=wd, capture_output=True, text=True, timeout=15)
                if ran.returncode == 0 and not ran.stderr and ran.stdout == f"{checks}\n":
                    result["status"] = "native"
                else:
                    result.update(status="failed", returncode=ran.returncode,
                                  stdout=ran.stdout, stderr=ran.stderr)
            print(result["status"], name, flush=True)
            results[name] = result
    args.result.write_text(json.dumps({"cpython_commit": reference["cpython_commit"],
        "denominator": reference["denominator"], "opt_level": args.opt_level,
        "tests": results}, indent=2) + "\n")
    return int(any(r["status"] != "native" for r in results.values()))


if __name__ == "__main__":
    sys.exit(main())
