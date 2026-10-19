# These buffered files expose cleanup of module owners and failed RHS temporaries.
# ruff: noqa: SIM115
from typing import TextIO

EARLIER = open("earlier.txt", "w", encoding="utf-8")
EARLIER.write("earlier buffer")


def buffered(path: str) -> TextIO:
    stream = open(path, "w", encoding="utf-8")
    stream.write("temporary buffer")
    return stream


def fail() -> TextIO:
    raise ValueError("initializer failed")


BROKEN = [buffered("temporary.txt"), fail()]
