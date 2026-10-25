"""Compare Unicode 16 str.lower over every native scalar, including final-sigma context."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile
import unicodedata


DRIVER = """import sys
from pathlib import Path
print(Path(sys.argv[1]).read_bytes().decode("utf-8").lower())
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or unicodedata.unidata_version != "16.0.0":
        parser.error("use the reference CPython build with Unicode 16.0.0")
    with tempfile.TemporaryDirectory(prefix="imre-unicode-lower-") as tmp:
        root = Path(tmp)
        entry = root / "lower.py"
        entry.write_text(DRIVER, encoding="utf-8")
        binaries = [root / "lower-O0", root / "lower-O3"]
        for level, binary in zip(("0", "3"), binaries, strict=True):
            subprocess.run([args.compiler.resolve(), "build", entry, "--runtime", args.runtime.resolve(),
                            "--clang", args.clang, "--opt-level", level, "--no-cache", "--quiet",
                            "-o", binary], check=True, timeout=120)
        path = root / "input"
        # NUL delimits independent inputs; it is neither cased nor case-ignorable.
        for prefix, suffix in [("", ""), ("A", "Σ"), (".", "Σ"), ("AΣ", "A"), ("AΣ", ".")]:
            source = "\0".join(prefix + chr(c) + suffix for c in range(0x110000)
                               if not 0xD800 <= c <= 0xDFFF)
            path.write_bytes(source.encode())
            expected = source.lower().encode() + b"\n"
            for binary in binaries:
                result = subprocess.run([binary, path], check=True, capture_output=True, timeout=300)
                if result.stdout != expected or result.stderr:
                    raise AssertionError(f"{binary.name} differs for context {prefix!r}, {suffix!r}")
        print("5,560,320 scalar/context cases matched per optimization level")


if __name__ == "__main__":
    main()
