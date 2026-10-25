"""Compare both modules' exception handlers on the combined native compiler and runtime."""
import argparse
from pathlib import Path
import os
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, required=True, type=lambda v: Path(v).resolve())
    args = parser.parse_args()
    source = Path(__file__).resolve().with_name("csv_exception_handlers.py")
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        file, pipe = work / "file", work / "fifo"
        file.write_text("unchanged")
        os.mkfifo(pipe)
        expected = subprocess.run([sys.executable, "-IB", source, file, pipe], cwd=tmp,
                                  check=True, capture_output=True, timeout=30)
        assert expected.stdout == b"csv\nsame file\nshutil base\nspecial file\nOS base\nCSV versus codec\ncodec versus CSV\n"
        for level in ("0", "3"):
            exe = work / "mixed-handlers"
            subprocess.run([args.imre, "build", source, "--no-cache", "--opt-level", level,
                            "--runtime", args.runtime, "--clang", args.clang, "-o", exe],
                           cwd=tmp, check=True, timeout=180)
            actual = subprocess.run([exe, file, pipe], cwd=tmp, check=True, capture_output=True,
                                    timeout=30)
            assert (actual.stdout, actual.stderr) == (expected.stdout, expected.stderr)
            assert file.read_text() == "unchanged"
            print(f"-O{level}: seven crossed exception handlers matched CPython")


if __name__ == "__main__":
    main()
