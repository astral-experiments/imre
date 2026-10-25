"""Compile complete CPython Excel-dialect reader methods and their native assertions."""
import argparse
import ast
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream

METHODS = (
    "test_single", "test_simple", "test_blankline", "test_empty_fields", "test_singlequoted",
    "test_singlequoted_left_empty", "test_singlequoted_right_empty", "test_single_quoted_quote",
    "test_quoted_quotes", "test_inline_quote", "test_inline_quotes", "test_quotes_and_more",
    "test_lone_quote", "test_quote_and_quote", "test_space_and_quote", "test_quoted",
    "test_quoted_quote", "test_dubious_quote",
)
ADAPTER = b"""import csv

class Case:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def readerAssertEqual(self, data: str, expected: list[list[str]]) -> None:
        with open("input.csv", "w", encoding="utf-8", newline="") as stream:
            stream.write(data)
        with open("input.csv", "r", encoding="utf-8", newline="") as stream:
            actual = list(csv.reader(stream))
        assert len(expected) == len(actual)
        for index in range(len(expected)):
            assert len(expected[index]) == len(actual[index])
            for column in range(len(expected[index])):
                assert actual[index][column] == expected[index][column]
        self.checks += 1

"""


def upstream_case():
    verify_upstream()
    data = (HERE / "upstream/Lib/test/test_csv.py").read_bytes()
    lines = data.splitlines(keepends=True)
    cls = next(n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == "TestDialectExcel")
    methods = {n.name: n for n in cls.body if isinstance(n, ast.FunctionDef)}
    source = bytearray(ADAPTER)
    for name in METHODS:
        method = methods[name]
        if method.decorator_list:
            raise ValueError(f"unsupported decorator on {name}")
        source.extend(b"".join(lines[method.lineno - 1:method.end_lineno]))
        source.extend(b"\n")
    source.extend(b"case = Case()\n")
    for name in METHODS:
        source.extend(f"case.{name}()\n".encode())
    # This method binds the local "input", a reserved builtin in Imre. Execute
    # the unmodified fixture on CPython and pass its arguments to the same native
    # reader and assertion adapter; the test method itself is counted separately.
    method = methods["test_quoted_nl"]
    body = b"".join(lines[method.lineno - 1:method.end_lineno]).decode()
    namespace = {}
    exec(compile("class Upstream:\n" + body, "test_csv.py", "exec"), namespace)
    class Fixture:
        def readerAssertEqual(self, data, expected):
            source.extend(f"case.readerAssertEqual({data!r}, {expected!r})\n".encode())
    namespace["Upstream"].test_quoted_nl(Fixture())
    source.extend(f"assert case.checks == {len(METHODS) + 1}\nprint(case.checks)\n".encode())
    return bytes(source)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="imre-csv-") as tmp:
        root = Path(tmp)
        upstream = root / "excel.py"
        upstream.write_bytes(upstream_case())
        for source in (upstream, *sorted((HERE.parents[1] / "tests/fixtures/csv").glob("*.py"))):
            expected = subprocess.run([args.python, "-I", "-B", source], cwd=root, capture_output=True, check=True)
            for level in args.opt_levels:
                binary = root / f"{source.stem}-{level}"
                flags = ["--sanitize"] if args.sanitize else []
                subprocess.run([args.imre.resolve(), "build", source, "--quiet", "--no-cache",
                                "--runtime", args.runtime.resolve(), "--clang", args.clang,
                                "--opt-level", level, *flags, "-o", binary], cwd=root, check=True)
                actual = subprocess.run([binary], cwd=root, capture_output=True, check=True)
                if (actual.stdout, actual.stderr) != (expected.stdout, expected.stderr):
                    raise AssertionError((level, source.name, actual.stdout, actual.stderr, expected.stdout, expected.stderr))
                print(f"O{level}: {len(METHODS)} native CPython methods and one CPython-driven multiline case" if source == upstream else
                      f"O{level}: {source.stem} CPython parity")


if __name__ == "__main__":
    main()
