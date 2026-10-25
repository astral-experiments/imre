import os
from pathlib import Path
from shutil import which


def name() -> str:
    print("cmd")
    return "input.txt"


def permissions() -> int:
    print("mode")
    return os.R_OK | os.W_OK


def search() -> str:
    print("path")
    return ":"


def lookup(cmd: bytes, path: bytes | None) -> bytes | None:
    return which(cmd, os.F_OK, path)


print(repr(which(cmd=name(), path=search(), mode=permissions())))
print(repr(which(mode=permissions(), path=search(), cmd=name())))
print(repr(which("input.txt", os.F_OK, "")))
print(repr(which("input.txt", mode=os.F_OK, path=":")))
print(repr(which(b"input.txt", os.R_OK, path=b"missing:.:.")))
print(repr(which(b"input.txt", os.X_OK, path=b".")))
print(repr(which("input.txt", os.R_OK, b"missing:.:.")))
print(repr(lookup(b"input.txt", b".")))
print(repr(lookup(b"./input.txt", None)))
print(repr(which(Path("./input.txt"), os.F_OK, ".")))
print(repr(which("tree", os.F_OK, ".")))
print(repr(which("nested///only//child.json", os.F_OK, "")))
print(repr(which("file-link", os.R_OK, ".")))
print(repr(which(b"bad\x00name", os.F_OK, b".")))
print(repr(which("input.txt", 8, ".")))
huge = 1 << 100
print(repr(which("missing", huge, ".")))
try:
    which("input.txt", huge, ".")
except OverflowError as error:
    print(str(error))
