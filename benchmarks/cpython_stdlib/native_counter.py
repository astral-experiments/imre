"""Compile complete pinned CPython Counter methods with native assertion adapters."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream
# These are dynamic assertion counts, including three stored keys with counts 10, -2, 0.
METHODS = {"test_total": 1, "test_invariant_for_the_in_operator": 6, "test_eq": 2,
           "test_le": 2, "test_lt": 2, "test_ge": 2, "test_gt": 2}


def assertion_fixture(method, class_name="Case"):
    ty = "int" if method == "test_total" else "Counter[str]"
    return f"""from collections import Counter

class {class_name}:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def assertTrue(self, value: bool) -> None:
        if not value:
            raise ValueError("assertTrue")
        self.checks += 1

    def assertIn(self, key: str, counts: Counter[str]) -> None:
        self.assertTrue(key in counts)

    def assertFalse(self, value: bool) -> None:
        self.assertTrue(not value)

    def assertEqual(self, left: {ty}, right: {ty}) -> None:
        self.assertTrue(left == right)

    def assertNotEqual(self, left: {ty}, right: {ty}) -> None:
        self.assertTrue(left != right)

"""


def source(method, body):
    return assertion_fixture(method).encode() + body + f"\ncase = Case()\ncase.{method}()\nprint(case.checks)\n".encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--opt-level", type=int, choices=range(4), default=3)
    args = parser.parse_args()
    commit = verify_upstream()
    data = (HERE / "upstream/Lib/test/test_collections.py").read_bytes()
    cls = next(node for node in ast.parse(data).body if isinstance(node, ast.ClassDef) and node.name == "TestCounter")
    tests = {}
    with tempfile.TemporaryDirectory() as tmp:
        for method, checks in METHODS.items():
            node = next(n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == method)
            if node.decorator_list:
                raise ValueError(f"unsupported upstream fixture for {method}")
            body = b"".join(data.splitlines(keepends=True)[node.lineno - 1:node.end_lineno])
            wd = Path(tmp) / method
            wd.mkdir()
            case = wd / "case.py"
            case.write_bytes(source(method, body))
            binary = wd / "case"
            built = subprocess.run([args.imre.resolve(), "build", case, "-o", binary, "--no-cache",
                "--opt-level", str(args.opt_level), "--runtime", args.runtime.resolve(), "--clang",
                args.clang.resolve()], cwd=wd, text=True, capture_output=True, timeout=120)
            result = built if built.returncode else subprocess.run([binary], cwd=wd, text=True, capture_output=True, timeout=30)
            passed = result.returncode == 0 and result.stdout == f"{checks}\n" and not result.stderr
            case_id = f"test.test_collections.TestCounter.{method}"
            tests[case_id] = {"method_sha256": hashlib.sha256(body).hexdigest(),
                              "assertions": checks, "status": "pass" if passed else "fail"}
            if not passed:
                tests[case_id]["diagnostic"] = result.stderr or repr(result.stdout)
                print(tests[case_id]["diagnostic"], file=sys.stderr)
            print(tests[case_id]["status"], case_id)
    args.result.write_text(json.dumps({"cpython_commit": commit, "opt_level": args.opt_level,
        "assertions": "native Imre", "tests": tests}, indent=2) + "\n")
    return int(any(test["status"] != "pass" for test in tests.values()))


if __name__ == "__main__":
    sys.exit(main())
