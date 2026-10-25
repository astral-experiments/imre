from pathlib import Path
from shutil import Error, SameFileError, copyfile

def source() -> Path:
    print("source")
    return Path("input.txt")

def destination() -> bytes:
    print("destination")
    return b"copied-bytes"

def follow() -> bool:
    print("follow")
    return False

target = copyfile(dst=destination(), follow_symlinks=follow(), src=source())
print(repr(target))
file = Path("copied again")
print(copyfile(target, file) == file)
print(file.read_bytes() == Path("input.txt").read_bytes())
print(copyfile(Path("file-link"), "copied link", follow_symlinks=False))
print(copyfile("dangling-link", b"copied dangling link", follow_symlinks=False))

try:
    copyfile(Path("input.txt"), "file-link")
except SameFileError as exc:
    print(str(exc))
try:
    copyfile(b"file-link", b"input.txt")
except Error as exc:
    print(str(exc))
try:
    copyfile(b"missing-bytes", b"created by mistake")
except FileNotFoundError as exc:
    print(str(exc))
    print(f"{exc!r}")
try:
    copyfile("input.txt", "missing directory/")
except FileNotFoundError as exc:
    print(str(exc))
    print(f"{exc!r}")
    cause = exc.__cause__
    print(exc.__suppress_context__)
    if cause is not None:
        print(isinstance(cause, IsADirectoryError))
        print(str(cause))
        print(f"{cause!r}")
try:
    copyfile("tree", "missing directory/")
except IsADirectoryError as exc:
    print(str(exc))
    print(f"{exc!r}")

for src, dst in [("input\x00", "copied"), ("input.txt", "copied\x00")]:
    try:
        copyfile(src, dst)
    except ValueError as exc:
        print(str(exc))
