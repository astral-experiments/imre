"""Compare the two real UTC certificate operations, range errors and source evaluation order."""

import argparse
from datetime import UTC, datetime, timedelta
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
CLOCK = b"""from datetime import datetime as dt
from datetime import timedelta
from datetime import timezone
def validity_window() -> None:
    print(dt.now(timezone.utc).isoformat())
    print((dt.now(timezone.utc) + timedelta(days=365)).isoformat())
    print(dt.now(timezone(timedelta(hours=5, minutes=30))).isoformat())
validity_window()
"""

# Local time and full datetime arithmetic are available in CPython.
REJECTED = [
    ("print(datetime.now())", 'missing required argument "tz"'),
    ("print(datetime.now(None))", "expected datetime.timezone, got None"),
    (
        "print(datetime(2000, 1, 1) - datetime(1999, 1, 1))",
        "unsupported arithmetic between these datetime values",
    ),
]


def run(argv):
    return subprocess.run(argv, capture_output=True, timeout=180)


def clock(argv):
    start = datetime.now(UTC)
    got = run(argv)
    end = datetime.now(UTC)
    assert got.returncode == 0 and got.stderr == b"", got
    a, b, east = map(datetime.fromisoformat, got.stdout.decode().splitlines())
    assert a.utcoffset() == b.utcoffset() == timedelta(0)
    assert east.utcoffset() == timedelta(hours=5, minutes=30)
    # Each native SystemTime observation must be in its actual parent-process wall interval.
    assert start <= a <= b - timedelta(days=365) <= east <= end, (
        start,
        a,
        b,
        east,
        end,
    )


def main():
    parser = argparse.ArgumentParser()
    for flag in ("imre", "runtime", "clang"):
        parser.add_argument("--" + flag, type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as temp:
        directory = Path(temp)
        live = directory / "live.py"
        live.write_bytes(CLOCK)
        clock([sys.executable, live])
        fixed = [HERE / "certificate_clock.py", HERE / "behavior.py"]
        reference = {src: run([sys.executable, src]) for src in fixed}
        for src, result in reference.items():
            assert result.returncode == 0 and result.stderr == b"", (src, result)
        for level in (0, 3):
            for reachable in (False, True):

                def build(src):
                    cmd = [
                        args.imre.resolve(),
                        "build",
                        src,
                        "--no-cache",
                        "--runtime",
                        args.runtime.resolve(),
                        "--clang",
                        args.clang.resolve(),
                        "--opt-level",
                        str(level),
                        "-o",
                        directory / "case",
                    ]
                    if reachable:
                        cmd.append("--reachable-only")
                    return run(cmd)

                for src in [*fixed, live]:
                    built = build(src)
                    assert built.returncode == 0, (level, reachable, src, built.stderr)
                    if src == live:
                        clock([directory / "case"])
                    else:
                        got = run([directory / "case"])
                        ref = reference[src]
                        assert (got.returncode, got.stdout, got.stderr) == (
                            ref.returncode,
                            ref.stdout,
                            ref.stderr,
                        ), (src, got, ref)
                for source, diagnostic in REJECTED:
                    unsupported = directory / "unsupported.py"
                    unsupported.write_text(
                        "from datetime import datetime\n" + source + "\n"
                    )
                    assert run([sys.executable, unsupported]).returncode == 0
                    blocked = build(unsupported)
                    assert (
                        blocked.returncode != 0
                        and diagnostic.encode() in blocked.stderr
                    ), (source, blocked.stderr)
                print(
                    f"O{level} {'reachable' if reachable else 'whole'}: fixed outputs, live UTC interval, and scope checks match"
                )


if __name__ == "__main__":
    main()
