"""Run Werkzeug's unchanged http_date against its wheel with local struct_time inputs."""

import argparse
import ast
import os
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
for dst in (0, 1, -1):
    print(http_date(struct_time((2024, 11, 3, 1, 30, 0, 0, 0, dst))))
    print(http_date(struct_time((2024, 3, 10, 2, 30, 0, 0, 0, dst))))
print(http_date(struct_time((1969, 12, 31, 23, 59, 59, 0, 0, -1))))
print(http_date(struct_time((1970, 1, 1, 0, 0, 0, 0, 0, -1))))
"""


def struct_time_branch(path):
    lines = path.read_bytes().splitlines(keepends=True)
    module = ast.parse(b"".join(lines))
    (function,) = [
        node
        for node in module.body
        if isinstance(node, ast.FunctionDef) and node.name == "http_date"
    ]
    condition, result = function.body[-2:]
    assert isinstance(condition, ast.If) and isinstance(result, ast.Return)
    assert ast.unparse(condition.test) == "isinstance(timestamp, struct_time)"
    assert (
        isinstance(condition.body[0], ast.Assign)
        and ast.unparse(condition.body[0].value) == "mktime(timestamp)"
    )
    return (
        b"from time import mktime, struct_time\nimport email.utils\n"
        b"def http_date(timestamp: struct_time | float) -> str:\n"
        + b"".join(lines[condition.lineno - 1 : result.end_lineno])
        + b"\n"
        + CALLS
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "wheelhouse"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("use the pinned CPython 3.14.5")
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        site = root / "site"
        site.mkdir()
        stage(args.wheelhouse, site)
        script = site / "http_date_reference.py"
        script.write_bytes(
            b"from werkzeug.http import http_date\nfrom time import struct_time\n"
            + CALLS
        )
        native = struct_time_branch(site / "werkzeug/http.py")
        for level in (0, 3):
            for reachable in (False, True):
                tool = Toolchain(args.imre, args.runtime, args.clang, level, reachable)
                binary = tool.build(native, root)
                for zone in ("UTC0", "EST5EDT,M3.2.0/2,M11.1.0/2"):
                    env = dict(os.environ, TZ=zone)
                    expected = subprocess.run(
                        [sys.executable, "-ESB", script],
                        env=env,
                        capture_output=True,
                        check=True,
                        timeout=30,
                    )
                    actual = subprocess.run(
                        [binary], env=env, capture_output=True, check=True, timeout=30
                    )
                    assert (actual.stdout, actual.stderr) == (
                        expected.stdout,
                        expected.stderr,
                    ), (actual, expected)
                print(
                    f"O{level} reachable={reachable}: eight calls to the original struct_time branch in both zones",
                    flush=True,
                )


if __name__ == "__main__":
    main()
