"""Negative control: BinaryIO does not promise getvalue on an ordinary file."""
from typing import BinaryIO


def snapshot(stream: BinaryIO) -> bytes:
    return stream.getvalue()


with open("example.dat", "rb") as source:
    print(repr(snapshot(source)))
