"""Compare every 16-bit POSIX mode with the accelerated CPython stat module."""

import argparse
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent))
from suite_native import Toolchain

PREDICATES = "DIR REG CHR BLK FIFO LNK SOCK PORT DOOR WHT".split()


def source():
    predicates = [
        f"int(stat.S_IS{name}(mode)) * {1 << i}" for i, name in enumerate(PREDICATES)
    ]
    return (
        "import stat\n"
        "def run() -> None:\n"
        "    for mode in range(65536):\n"
        "        flags = " + " + ".join(predicates) + "\n"
        "        print(str(stat.S_IFMT(mode)) + ':' + str(stat.S_IMODE(mode)) + ':' + str(flags) + ':' + stat.filemode(mode))\n"
        "run()\n"
    ).encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, type=Path, required=True)
    a = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("use CPython 3.14.5")
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        program = source()
        reference = subprocess.run(
            [sys.executable, "-I", "-"], input=program, capture_output=True, check=True
        )
        if reference.stderr or reference.stdout.count(b"\n") != 65536:
            raise ValueError("incomplete CPython reference")
        for level in (0, 3):
            tool = Toolchain(
                a.imre.resolve(), a.runtime.resolve(), a.clang.resolve(), level
            )
            binary = tool.build(program, work)
            actual = subprocess.run(
                [binary], capture_output=True, timeout=120, check=True
            )
            if actual.stderr or actual.stdout != reference.stdout:
                for mode, (expected, observed) in enumerate(
                    zip(reference.stdout.splitlines(), actual.stdout.splitlines())
                ):
                    if expected != observed:
                        raise AssertionError((level, oct(mode), expected, observed))
                raise AssertionError(
                    (level, len(reference.stdout), len(actual.stdout), actual.stderr)
                )
            print(
                f"O{level}: 65536/65536 modes match, sha256={hashlib.sha256(actual.stdout).hexdigest()}",
                flush=True,
            )


if __name__ == "__main__":
    main()
