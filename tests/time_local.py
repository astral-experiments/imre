"""Compare local-time normalization and struct_time fields with the platform's CPython."""

import argparse
import difflib
import os
from pathlib import Path
import subprocess
import sys
import tempfile

SOURCE = (
    Path(__file__).resolve().parents[1]
    / "benchmarks/cpython_stdlib/full/time/operations.py"
)
ZONES = ("UTC0", "EST5EDT,M3.2.0/2,M11.1.0/2")


def output(command, zone):
    result = subprocess.run(
        command,
        env={**os.environ, "TZ": zone},
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode or result.stderr:
        raise AssertionError(
            (command, zone, result.returncode, result.stdout, result.stderr)
        )
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    expected = {zone: output([sys.executable, "-EB", SOURCE], zone) for zone in ZONES}
    assert len(set(expected.values())) == len(ZONES), (
        "both processes produced the same timezone results"
    )
    with tempfile.TemporaryDirectory() as tmp:
        binary = Path(tmp) / "time_local"
        for level in args.opt_levels:
            for reachable in (False, True):
                cmd = [
                    args.compiler.resolve(),
                    "build",
                    SOURCE,
                    "--no-cache",
                    "--opt-level",
                    level,
                    "-o",
                    binary,
                    "--runtime",
                    args.runtime.resolve(),
                    "--clang",
                    args.clang,
                ]
                if reachable:
                    cmd.append("--reachable-only")
                if args.sanitize:
                    cmd.append("--sanitize")
                subprocess.run(cmd, check=True, timeout=180)
                for zone, reference in expected.items():
                    actual = output([binary], zone)
                    if reference != actual:
                        raise AssertionError(
                            "".join(
                                difflib.unified_diff(
                                    reference.splitlines(True),
                                    actual.splitlines(True),
                                    "CPython",
                                    "Imre",
                                )
                            )
                        )
                print(
                    f"O{level} reachable={reachable} matched CPython in both timezones",
                    flush=True,
                )


if __name__ == "__main__":
    main()
