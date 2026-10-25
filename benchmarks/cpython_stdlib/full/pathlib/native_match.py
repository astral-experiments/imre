"""Run the complete CPython matching methods against compiled POSIX Path objects."""
import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BENCHMARK = HERE.parents[1]
sys.path.insert(0, str(BENCHMARK))
from upstream import verify_upstream

PACKAGE = BENCHMARK / "upstream/Lib/test/test_pathlib"
METHODS = {
    ("test_join", "JoinTestBase", "test_full_match"): {"PathJoinTest": "Path"},
    **{("test_pathlib", "PurePathTest", method): {"PathTest": "Path", "PosixPathTest": "PosixPath"}
       for method in ("test_full_match_case_sensitive", "test_match_common", "test_match_posix")},
}


def method_source(module, owner, name):
    data = (PACKAGE / f"{module}.py").read_bytes()
    cls = next(n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == owner)
    method = next(n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == name)
    # @needs_posix on test_match_posix only registers a fixture precondition.
    # Check it before compiling; the method and all its operands stay byte-identical.
    if [ast.unparse(d) for d in method.decorator_list] != (["needs_posix"] if name == "test_match_posix" else []):
        raise ValueError(f"unexpected upstream decorators for {name}")
    body = b"".join(data.splitlines(keepends=True)[method.lineno - 1:method.end_lineno])
    checks = sum(isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute)
                 and n.func.attr in ("assertTrue", "assertFalse") for n in ast.walk(method))
    if not checks:
        raise ValueError(f"missing native assertions: {name}")
    return body, checks


def native_source():
    source = bytearray(b"from pathlib import Path, PosixPath\n")
    rows = {}
    invocations = []
    for (module, owner, method), fixtures in METHODS.items():
        body, checks = method_source(module, owner, method)
        for fixture, constructor in fixtures.items():
            case_id = f"test.test_pathlib.{module}.{fixture}.{method}"
            name = f"{fixture}_{method}"
            source.extend(f"""
class {name}:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def cls(self, spelling: str) -> {constructor}:
        return {constructor}(spelling)

    def assertTrue(self, value: bool) -> None:
        if not value:
            raise AssertionError("expected True")
        self.checks += 1

    def assertFalse(self, value: bool) -> None:
        if value:
            raise AssertionError("expected False")
        self.checks += 1

""".encode())
            source.extend(body)
            invocations.append(f'case_{name} = {name}()\ncase_{name}.{method}()\n'
                               f'print(f"{case_id} {{case_{name}.checks}}")\n')
            rows[case_id] = {"assertions": checks, "method_sha256": hashlib.sha256(body).hexdigest()}
    source.extend("\n".join(invocations).encode())
    return source, rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, required=True, type=Path)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("the concrete fixtures in this program require POSIX")
    commit = verify_upstream()
    source, rows = native_source()
    with tempfile.TemporaryDirectory() as tmp:
        script, binary = Path(tmp) / "methods.py", Path(tmp) / "methods"
        script.write_bytes(source)
        subprocess.run([args.imre.resolve(), "build", script, "--no-cache", "--opt-level",
                        args.opt_level, "--runtime", args.runtime.resolve(), "--clang",
                        args.clang.resolve(), "-o", binary], check=True, timeout=180)
        actual = subprocess.run([binary], capture_output=True, text=True, check=True, timeout=60)
        expected = "".join(f"{name} {row['assertions']}\n" for name, row in rows.items())
        if actual.stdout != expected or actual.stderr:
            raise AssertionError(f"native methods: {actual.stdout!r}, {actual.stderr!r}")
    args.result.write_text(json.dumps({"cpython_commit": commit, "opt_level": int(args.opt_level),
        "execution": "native-body-and-operations", "passed": rows}, indent=2) + "\n")
    print(f"{len(rows)} native methods; {sum(r['assertions'] for r in rows.values())} assertions")


if __name__ == "__main__":
    main()
