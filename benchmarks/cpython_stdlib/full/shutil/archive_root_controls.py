"""Check which original filename make_archive attaches to real root errors."""
import argparse
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory


PROGRAM = r'''import shutil
from pathlib import Path, PurePosixPath, PureWindowsPath

def explicit(root: Path, fmt: str, dry_run: bool) -> None:
    try:
        shutil.make_archive("archive", fmt, root_dir=root, dry_run=dry_run)
    except FileNotFoundError as error:
        name = error.filename
        print(repr(error))
        print(repr(name))
        print(isinstance(name, str))
    except NotADirectoryError as error:
        name = error.filename
        print(repr(error))
        print(repr(name))
        print(isinstance(name, Path) and name is root)

def choice(root: str | Path | None, fmt: str, dry_run: bool) -> None:
    try:
        shutil.make_archive("archive", fmt, root_dir=root, dry_run=dry_run)
    except NotADirectoryError as error:
        name = error.filename
        print(repr(name))
        if isinstance(root, Path):
            print(isinstance(name, Path) and name is root)
        elif isinstance(root, str):
            print(isinstance(name, str) and name is root)
    except FileNotFoundError as error:
        name = error.filename
        print(repr(name))
        print(isinstance(name, str))

for format in ("tar", "zip"):
    for dry in (False, True):
        explicit(Path("missing"), format, dry)
        explicit(Path("plain"), format, dry)
        choice("missing", format, dry)
        choice("plain", format, dry)
        choice(Path("missing"), format, dry)
        choice(Path("plain"), format, dry)
    try:
        shutil.make_archive(Path("archive"), format, dry_run=True)
    except TypeError as error:
        print(str(error))
    try:
        shutil.make_archive(PurePosixPath("archive"), format, dry_run=True)
    except TypeError as error:
        print(str(error))
    try:
        shutil.make_archive(PureWindowsPath("archive"), format, dry_run=True)
    except TypeError as error:
        print(str(error))
    try:
        shutil.make_archive(PureWindowsPath("archive"), format, root_dir="bad\0root", dry_run=True)
    except ValueError as error:
        print(str(error))
    try:
        shutil.make_archive("archive", format, root_dir=PurePosixPath("bad\0root"), dry_run=True)
    except ValueError as error:
        print(str(error))
    try:
        shutil.make_archive("archive", "not-registered", root_dir="bad\0root", dry_run=True)
    except ValueError as error:
        print(str(error))
    print(Path(shutil.make_archive(PurePosixPath("archive"), format, root_dir=".", dry_run=True)).name)
    print(Path(shutil.make_archive(PureWindowsPath("archive"), format, root_dir=".", dry_run=True)).name)
'''


REMOVED_CWD = '''import shutil
from pathlib import Path

Path("../vanished").rmdir()
for format in ("no-such-format", "zip"):
    try:
        shutil.unpack_archive("packet.unknown", format=format)
    except FileNotFoundError as error:
        print(repr(error.errno) + " " + str(error.filename is None) + " " + str(error.filename2 is None))
try:
    shutil.unpack_archive("packet.unknown")
except FileNotFoundError as error:
    print(repr(error.errno) + " " + str(error.filename is None) + " " + str(error.filename2 is None))
try:
    shutil.unpack_archive("packet.unknown", "", "no-such-format")
except ValueError as error:
    print(str(error))
try:
    shutil.unpack_archive("packet.unknown", "somewhere")
except shutil.ReadError as error:
    print(str(error))
'''


def removed_cwd(command, root):
    vanished = root / "vanished"
    vanished.mkdir()
    child = subprocess.run(command, cwd=vanished, capture_output=True, check=True, timeout=30)
    assert not child.stderr and not vanished.exists(), child.stderr
    return child.stdout


def observe(command, directory):
    child = subprocess.run(command, cwd=directory, capture_output=True, check=True, timeout=30)
    assert not child.stderr, child.stderr
    assert not list(directory.glob("archive.*"))
    return child.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, type=lambda p: Path(p).resolve())
    parser.add_argument("--oracle-only", action="store_true")
    args = parser.parse_args()
    if not args.oracle_only and not all((args.imre, args.runtime, args.clang)):
        parser.error("supply --imre, --runtime and --clang for the native comparison")
    with TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "plain").write_bytes(b"not a directory")
        source = root / "root_errors.py"
        source.write_text(PROGRAM)
        cwd_source = root / "unpack_cwd.py"
        cwd_source.write_text(REMOVED_CWD)
        cwd_expected = removed_cwd([sys.executable, "-IB", cwd_source], root)
        assert cwd_expected == (b"2 True True\n" * 3 + b"Unknown unpack format 'no-such-format'\n"
                                + b"Unknown archive format 'packet.unknown'\n")
        expected = observe([sys.executable, "-IB", source], root)
        assert expected.count(b"True\n") == 24 and b"False\n" not in expected
        for cls in (b"PosixPath", b"PurePosixPath", b"PureWindowsPath"):
            assert expected.count(b"unsupported operand type(s) for +: '" + cls + b"' and 'str'\n") == 2
        assert expected.count(b"stat: embedded null character in path\n") == 4
        assert expected.count(b"unknown archive format 'not-registered'\n") == 2
        assert expected.count(b"archive.tar\n") == expected.count(b"archive.zip\n") == 2
        if args.oracle_only:
            print("CPython: 40 make_archive path controls and five removed-cwd unpack controls passed")
            return
        for level in (0, 3):
            binary = root / "native"
            subprocess.run([args.imre, "build", source, "--no-cache", "--runtime", args.runtime,
                            "--clang", args.clang, "--opt-level", str(level), "-o", binary], check=True)
            assert observe([binary], root) == expected
            subprocess.run([args.imre, "build", cwd_source, "--no-cache", "--runtime", args.runtime,
                            "--clang", args.clang, "--opt-level", str(level), "-o", binary], check=True)
            assert removed_cwd([binary], root) == cwd_expected
            print(f"-O{level}: 40 make_archive path controls and five removed-cwd unpack controls matched CPython")


if __name__ == "__main__":
    main()
