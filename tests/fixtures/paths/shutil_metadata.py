from pathlib import Path
from shutil import _copyxattr, copy, copy2, copymode, copystat

def describe(path: str | Path) -> None:
    if isinstance(path, Path):
        print("Path")
        print(str(path))
    else:
        print("str")
        print(path)

describe(copy("input.txt", Path("copied file")))
describe(copy2("input.txt", Path("empty-directory")))
print(copy2(b"empty.txt", b"empty-directory"))
print(copymode("file-link", "copied file", follow_symlinks=False))
print(copystat(src="input.txt", dst="copied file"))
try:
    copy("file-link", b"empty-directory")
except TypeError as exc:
    print(str(exc))

try:
    copystat("missing-source", "bad\0destination")
except FileNotFoundError:
    print("missing source")
try:
    copymode("input.txt/child", "bad\0destination")
except NotADirectoryError:
    print("source traverses a file")
try:
    copy2("bad\0source", b"empty-directory")
except TypeError:
    print("mixed directory and basename types")
try:
    copystat("input.txt", "bad\0destination")
except ValueError:
    print("invalid destination")
print(_copyxattr("empty.txt", "bad\0destination"))
