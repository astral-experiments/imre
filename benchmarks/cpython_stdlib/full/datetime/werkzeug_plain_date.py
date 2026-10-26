"""Compare the pinned Werkzeug plain-date branch and UTC normalization with CPython."""
import argparse
import ast
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2] / "flask"))
from gate import stage
sys.path.insert(0, str(HERE.parents[1]))
from suite_native import Toolchain

CALLS = b"""
for y, m, d in [(1, 1, 1), (1899, 12, 31), (1900, 3, 1), (1970, 1, 1),
                (2000, 2, 29), (2026, 10, 26), (9999, 12, 31)]:
    print(plain(date(y, m, d)))
"""

UTC_OBSERVE = b"""
def observe(value: datetime | None) -> None:
    actual = _dt_as_utc(value)
    if actual is None:
        print("None")
    else:
        print((actual.isoformat(), actual.fold, actual is value, actual.tzinfo is timezone.utc))
"""
UTC_CALLS = b"""
observe(None)
observe(datetime(2020, 2, 29, 23, 59, fold=1))
observe(datetime(2020, 3, 1, tzinfo=timezone.utc, fold=1))
observe(datetime(2020, 3, 1, tzinfo=timezone(timedelta(0), "named zero"), fold=1))
observe(datetime(2020, 3, 1, microsecond=1, tzinfo=timezone(timedelta(seconds=2, microseconds=3)), fold=1))
"""


def plain_date_branch(path):
    lines = path.read_bytes().splitlines(keepends=True)
    module = ast.parse(b"".join(lines))
    wrapper, = [node for node in module.body if isinstance(node, ast.FunctionDef) and node.name == "http_date"]
    outer = next(node for node in wrapper.body if isinstance(node, ast.If))
    inner, result = outer.body
    # Keep both original guards and the assignment. Inside the plain-date
    # branch, the original return is the next executed statement.
    assert isinstance(inner, ast.If) and isinstance(result, ast.Return)
    return (b"from datetime import date, datetime, time, timedelta, timezone\nimport email.utils\n"
            b"def plain(timestamp: date) -> str:\n"
            + b"".join(lines[outer.lineno - 1:inner.body[-1].end_lineno])
            + b"    " + lines[result.lineno - 1]
            + b"    raise ValueError('plain date required')\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "wheelhouse"):
        parser.add_argument("--" + flag, type=Path, required=True)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("use the pinned CPython 3.14.5")
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        site = root / "site"
        site.mkdir()
        stage(args.wheelhouse, site)
        reference = site / "reference.py"
        reference.write_bytes(b"from datetime import date, datetime, timedelta, timezone\nfrom werkzeug.http import http_date as plain\n"
                              b"from werkzeug._internal import _dt_as_utc\n" + UTC_OBSERVE + CALLS + UTC_CALLS)
        expected = subprocess.run([sys.executable, "-ESB", reference], check=True, capture_output=True, timeout=30)
        native = plain_date_branch(site / "werkzeug/http.py")
        internal = (site / "werkzeug/_internal.py").read_bytes()
        implementation, = [node for node in ast.parse(internal).body
                           if isinstance(node, ast.FunctionDef) and node.name == "_dt_as_utc" and not node.decorator_list]
        native += b"\n" + b"".join(internal.splitlines(keepends=True)[implementation.lineno - 1:implementation.end_lineno])
        native += UTC_OBSERVE + CALLS + UTC_CALLS
        for level in (0, 3):
            for reachable in (False, True):
                tools = Toolchain(args.imre, args.runtime, args.clang, level, reachable)
                binary = tools.build(native, root)
                actual = subprocess.run([binary], check=True, capture_output=True, timeout=30)
                assert (actual.stdout, actual.stderr) == (expected.stdout, expected.stderr), (actual, expected)
                print(f"O{level} {'reachable' if reachable else 'whole'}: seven plain dates and five original _dt_as_utc calls matched")


if __name__ == "__main__":
    main()
