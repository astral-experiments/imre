import os
import os.path as path
from os import path as imported
from os.path import splitext
from pathlib import Path
from posixpath import join as lexical_join


def next_bytes(data: str) -> bytes:
    print(data)
    return b"tail"


def later_failure() -> str:
    print("late argument")
    raise ValueError("later")


def remove_first(parts: list[str]) -> str:
    parts.pop()
    return "last"


parts = ["a//../b"]
print(lexical_join(parts[0], remove_first(parts)))
for spelling in ["", "/", "//", "///", "a//b/", ".dot", ".x...y.", "λ/尾.δ", "a\0b"]:
    print(repr(path.basename(p=spelling)))
    print(repr(os.path.dirname(spelling)))
    print(repr(splitext(p=spelling)))
    # POSIX accepts a=, while the cross-platform typeshed signature is positional-only.
    print(repr(imported.join(a=spelling)))  # ty: ignore[no-matching-overload]
for raw in [b"", b"////", b"\xff/\x80.name", b".\xff\0..", b"/\xff/./"]:
    print(repr(path.join(raw, b"", b"\xff")))
    print(repr(path.basename(raw)))
    print(repr(path.dirname(raw)))
    print(repr(path.splitext(raw)))

print(os.path.join("./raw//", Path("a/./b"), ""))
print(os.path.basename(Path("a//b/")))
print(os.path.dirname(Path("one")))
print(repr(os.path.splitext(Path("//δ/.file.ext"))))
try:
    print(path.join("raw", next_bytes("middle"), b"last"))  # ty: ignore[no-matching-overload]
except TypeError as error:
    print(str(error))
try:
    print(path.join(b"raw", Path("/absolute")))  # ty: ignore[no-matching-overload]
except TypeError as error:
    print(str(error))
try:
    print(path.join(Path("first"), b"bytes"))  # ty: ignore[no-matching-overload]
except TypeError as error:
    print(str(error))
try:
    print(path.join(b"first", "second", Path("third")))  # ty: ignore[no-matching-overload]
except TypeError as error:
    print(str(error))
try:
    print(path.join(b"raw", "bad", later_failure()))  # ty: ignore[no-matching-overload]
except ValueError as error:
    print(str(error))
