"""Run complete upstream pathlib test methods as standalone Relar programs."""
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

TESTS = (
    ("test_join", "PathJoinTest", "test_name", "str"),
    ("test_join", "PathJoinTest", "test_stem", "str"),
    ("test_join", "PathJoinTest", "test_suffix", "str"),
    ("test_join", "PathJoinTest", "test_parent", "Path"),
    ("test_join_posix", "PosixPathJoinTest", "test_div", "Path"),
    ("test_join_posix", "PosixPathJoinTest", "test_join", "Path"),
)


def native_source(module, method, value_type):
    path = HERE / "upstream/Lib/test/test_pathlib" / f"{module}.py"
    data = path.read_bytes()
    base = next(node for node in ast.parse(data).body
                if isinstance(node, ast.ClassDef) and node.name == "JoinTestBase")
    test = next(node for node in base.body if isinstance(node, ast.FunctionDef) and node.name == method)
    if test.decorator_list:
        raise ValueError(f"unsupported upstream fixture for {method}")
    # Copy the whole upstream method, including its operands and expected values.
    body = b"".join(data.splitlines(keepends=True)[test.lineno-1:test.end_lineno])
    checks = sum(isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute)
                 and n.func.attr == "assertEqual" for n in ast.walk(test))
    adapter = f"""from pathlib import Path

class Case:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def cls(self, spelling: str) -> Path:
        return Path(spelling)

    def assertEqual(self, actual: {value_type}, expected: {value_type}) -> None:
        if actual != expected:
            raise ValueError(f"expected {{expected}}, got {{actual}}")
        self.checks += 1

"""
    run = f"\ncase = Case()\ncase.{method}()\nprint(case.checks)\n"
    return adapter.encode() + body + run.encode(), checks, hashlib.sha256(body).hexdigest()


def run(argv, cwd):
    return subprocess.run(argv, cwd=cwd, capture_output=True, text=True, timeout=120)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--relar", required=True, type=Path)
    parser.add_argument("--runtime", type=Path)
    parser.add_argument("--clang", type=Path)
    parser.add_argument("--opt-level", type=int, choices=(0, 1, 2, 3), default=0)
    parser.add_argument("--result", type=Path)
    args = parser.parse_args()
    if sys.platform not in ("darwin", "linux"):
        parser.error("the selected upstream fixture requires the native POSIX Path type")
    provenance = verify_upstream()
    command = [args.relar.resolve(), "build", "--no-cache", "--opt-level", str(args.opt_level)]
    if args.runtime:
        command.extend(["--runtime", args.runtime.resolve()])
    if args.clang:
        command.extend(["--clang", args.clang.resolve()])
    results = {}
    with tempfile.TemporaryDirectory() as tmp:
        for module, fixture, method, value_type in TESTS:
            case_id = f"test.test_pathlib.{module}.{fixture}.{method}"
            source, checks, digest = native_source(module, method, value_type)
            wd = Path(tmp) / case_id
            wd.mkdir()
            (wd / "case.py").write_bytes(source)
            compiled = run([command[0], command[1], "case.py", *command[2:], "-o", wd / "case"], wd)
            record = {"method_sha256": digest, "assertions": checks}
            if compiled.returncode:
                record.update(status="compile_error", diagnostic=compiled.stderr)
            else:
                output = run([wd / "case"], wd)
                if checks and output.returncode == 0 and not output.stderr and output.stdout == f"{checks}\n":
                    record.update(status="pass")
                else:
                    record.update(status="runtime_error", stdout=output.stdout, stderr=output.stderr,
                                  returncode=output.returncode)
            results[case_id] = record
            print(f"{record['status']:13s} {case_id} ({checks} assertions)")
            if record["status"] != "pass":
                print(record.get("diagnostic", record.get("stderr", "")), file=sys.stderr)
    if args.result:
        args.result.write_text(json.dumps({"cpython_commit": provenance,
            "opt_level": args.opt_level, "tests": results}, indent=2) + "\n")
    return int(any(value["status"] != "pass" for value in results.values()))


if __name__ == "__main__":
    sys.exit(main())
