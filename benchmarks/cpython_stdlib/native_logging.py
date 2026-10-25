"""Compile complete CPython logger test methods and verify their stderr and identity assertions."""
import argparse
import ast
import io
import re
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream

METHODS = {
    "BuiltinLevelsTest": ("test_nested_explicit", "test_nested_inherited",
                          "test_nested_with_virtual_parent"),
    "LoggerTest": ("test_root_logger_aliases",),
}

FIXTURE = '''import logging

class Case:
    sequence: int
    checks: int

    def __init__(self) -> None:
        self.sequence = 0
        self.checks = 0

    def next_message(self) -> str:
        self.sequence += 1
        return str(self.sequence)

    def assertIs(self, left: logging.Logger | None, right: logging.Logger | None) -> None:
        if left is not right:
            raise AssertionError("logger identity")
        self.checks += 1

    def assertIsNot(self, left: logging.Logger | None, right: logging.Logger | None) -> None:
        if left is right:
            raise AssertionError("logger identity")
        self.checks += 1

    def assert_log_lines(self, expected: list[tuple[str, str, str]]) -> None:
        for name, level, message in expected:
            print(name + "\\t" + level + "\\t" + message)
        self.checks += 1

'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--opt-level", choices=("0", "1", "2", "3"), default="3")
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use CPython 3.14.5 for the original unittest assertions")
    verify_upstream()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = HERE / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_logging
    upstream = vendored / "test_logging.py"
    assert Path(test_logging.__file__).resolve() == upstream
    original = upstream.read_text()
    lines = original.splitlines(keepends=True)
    classes = {node.name: node for node in ast.parse(original).body if isinstance(node, ast.ClassDef)}
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        binary = tmp / "logger-case"
        def build(path):
            subprocess.run([args.imre.resolve(), "build", path, "-o", binary, "--no-cache",
                            "--opt-level", args.opt_level, "--runtime", args.runtime.resolve(),
                            "--clang", args.clang.resolve(), *(["--sanitize"] if args.sanitize else [])],
                           cwd=tmp, check=True, timeout=120)
        def run(argv):
            result = subprocess.run(argv, cwd=tmp, capture_output=True, text=True, timeout=30)
            if result.returncode:
                raise RuntimeError(f"{argv[0]} exited {result.returncode}\n{result.stderr}")
            return result

        basic = HERE.parents[1] / "tests/fixtures/logging/basic.py"
        for source in (basic, HERE / "full/logging/record_delivery.py"):
            build(source)
            expected, actual = run([sys.executable, source]), run([binary])
            assert (expected.stdout, expected.stderr) == (actual.stdout, actual.stderr), source
        timestamps = basic.with_name("timestamps.py")
        build(timestamps)
        for result in (run([sys.executable, timestamps]), run([binary])):
            rows = result.stderr.splitlines()
            assert not result.stdout and len(rows) == 24
            for row in rows:
                times = row.removesuffix("|").split("|")
                assert len(times) == 768 and len(set(times)) == 1
                assert re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2},[0-9]{3}", times[0])

        source = tmp / "case.py"
        for cls, methods in METHODS.items():
            for name in methods:
                method = next(n for n in classes[cls].body
                              if isinstance(n, ast.FunctionDef) and n.name == name)
                assert not method.decorator_list
                body = "".join(lines[method.lineno - 1:method.end_lineno])
                count = sum(isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute)
                            and n.func.attr in {"assertIs", "assertIsNot", "assert_log_lines"}
                            for n in ast.walk(method))
                source.write_text(FIXTURE + body + "\nlogging.basicConfig(level=logging.DEBUG, "
                                  + f"format={test_logging.BaseTest.log_format!r})\ncase = Case()\n"
                                  + f"case.{name}()\nprint(case.checks)\n")
                build(source)
                actual = run([binary])
                rows = actual.stdout.splitlines()
                assert rows.pop() == str(count) and count > 0
                expected = [tuple(row.split("\t")) for row in rows]
                # The original method supplied these expected rows. Its upstream
                # unittest helper compares every field to the native stderr.
                stream = io.StringIO()
                stream.write(actual.stderr)
                test_logging.BaseTest().assert_log_lines(expected, stream)
                print("pass", f"test.test_logging.{cls}.{name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
