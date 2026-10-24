import os.path
import shutil
import sys
import sysconfig
from pathlib import Path


def joined(root: str | Path, tail: str | Path) -> str:
    return os.path.join(root, tail, "")


def mixed(tail: str | Path) -> None:
    try:
        os.path.join(b"root", tail)  # ty: ignore[no-matching-overload]
    except TypeError as error:
        print(str(error))


print(sys.platform)
print(Path(sysconfig.get_path("scripts")).name)
for first in ["", ".", "/", "//", "./a//"]:
    for tail in ["", ".", "/absolute", "relative//tail"]:
        print(os.path.join(first, tail))
print(os.path.join(Path("a/b"), "c", "", "../..", ""))
print(joined("./root//", Path("a/b")))
print(joined(Path("a/b"), "./tail//"))
mixed("tail")
mixed(Path("tail"))
for path in ["", ".", "input.txt", "input.txt/", "file-link", "dangling-link", "bad\0name"]:
    print(os.path.exists(path))
for program in ["./command", "./input.txt", "./tree", "./missing", "tree/../command", "relar-missing-command"]:
    print(repr(shutil.which(program)))
print("missing executable", sys.stderr)
