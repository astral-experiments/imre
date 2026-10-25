"""Compare real symlink-creation errors for arbitrary POSIX filenames."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from copyfile import build

SOURCE = b'''from shutil import copyfile

for link in ("both", "single", "double", "plain"):
    try:
        copyfile(link, "destination", follow_symlinks=False)
    except FileExistsError as exc:
        print(str(exc))
        print(repr(exc))
        print(repr(exc.filename2))
        print(repr(exc.errno))
for raw_link in (b"both", b"single", b"double", b"plain"):
    try:
        copyfile(raw_link, b"destination", follow_symlinks=False)
    except FileExistsError as exc:
        print(str(exc))
        print(repr(exc.filename))
        print(repr(exc.filename2))
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, type=lambda s: Path(s).resolve(), required=True)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or os.name != "posix":
        parser.error("requires CPython 3.14.5 on POSIX")
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        targets = {
            "both": b"'\xff\"\xc3\xa9\n", "single": b"a'\xfe\\udcfe",
            "double": b'\"\xed\xa0\x80', "plain": b"\xff\x80\xef\xbf\xbd",
        }
        for link, target in targets.items():
            os.symlink(target, root / link)
        dest = root / "destination"
        dest.write_bytes(b"untouched")
        script = root / "errors.py"
        script.write_bytes(SOURCE)

        def run(command):
            process = subprocess.run(command, cwd=root, capture_output=True, check=True)
            assert dest.read_bytes() == b"untouched"
            assert {k: os.readlink(os.fsencode(root / k)) for k in targets} == targets
            return process.stdout, process.stderr

        expected = run([sys.executable, "-IB", script])
        assert len(expected[0].splitlines()) == 28
        for level in (0, 3):
            exe = build(args, root, f"native-{level}", SOURCE, level)
            assert run([exe]) == expected
            print(f"-O{level}: all eight failing symlink copies matched CPython")


if __name__ == "__main__":
    main()
