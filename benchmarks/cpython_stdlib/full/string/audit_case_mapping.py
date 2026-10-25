"""Compare the linked native case mappings with CPython for all Unicode scalars."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--cc", default="clang")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("the reference must be CPython 3.14.5")
    with tempfile.TemporaryDirectory() as directory:
        exe = Path(directory) / "audit"
        subprocess.run([args.cc, "-O2", HERE / "case_mapping_audit.c", args.runtime.resolve(),
                        "-ldl", "-lm", "-lpthread", "-o", exe], check=True)
        with subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE) as run:
            for cp in range(0x110000):
                if 0xD800 <= cp <= 0xDFFF:
                    continue  # Imre strings hold Unicode scalar values.
                for s in (chr(cp), chr(cp) + "Σ\u0301", "AΣ" + chr(cp) + "A"):
                    data = s.encode(), s.capitalize().encode(), s.lower().encode()
                    run.stdin.write(bytes(map(len, data)) + b"".join(data))
            run.stdin.close()
            result = run.stdout.read()
            if run.wait() or result != b"3336192 lowercase and capitalize pairs matched\n":
                raise AssertionError(result)
            print(result.decode(), end="")


if __name__ == "__main__":
    main()
