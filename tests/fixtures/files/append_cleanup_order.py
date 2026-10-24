"""Buffered writes expose the order in which the last file owners are dropped."""

from pathlib import Path
from typing import BinaryIO


def buffered(path: str, data: bytes) -> BinaryIO:
    stream = open(path, "ab")
    stream.write(data)
    return stream


def receiver(path: str) -> list[BinaryIO]:
    return [buffered(path, b"A"), buffered(path, b"B")]


def failing() -> BinaryIO:
    raise ValueError("item failed")


receiver("temporary").append(buffered("temporary", b"C"))
print(Path("temporary").read_text(encoding="utf-8"))

stable = receiver("stable")
stable.append(buffered("stable", b"C"))
stable = []
print(Path("stable").read_text(encoding="utf-8"))

try:
    receiver("raised").append(failing())
except ValueError as error:
    print(str(error))
print(Path("raised").read_text(encoding="utf-8"))
