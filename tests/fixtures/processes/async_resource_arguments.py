"""A suspended call transfers buffered handles to its parameters on entry."""

import asyncio
from collections.abc import Awaitable, Callable
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO


def buffered(path: str, data: bytes) -> BinaryIO:
    stream = open(path, "ab")
    stream.write(data)
    return stream


async def rebind(left: BinaryIO, right: BinaryIO, path: str) -> None:
    left = right
    print(Path(path).read_text())
    left.write(b"C")


@dataclass
class Output:
    path: str

    @contextmanager
    def opened(self, left: BinaryIO, right: BinaryIO) -> None:
        left = right
        print(Path(self.path).read_text())
        try:
            yield
        finally:
            left.write(b"C")


async def exercise() -> None:
    await rebind(buffered("direct", b"A"), buffered("direct", b"B"), "direct")
    callback: Callable[[BinaryIO, BinaryIO, str], Awaitable[None]] = rebind
    await callback(buffered("indirect", b"A"), buffered("indirect", b"B"), "indirect")
    shared = buffered("shared", b"A")
    await callback(shared, shared, "shared")
    shared.write(b"D")
    shared.close()


asyncio.run(exercise())
with Output("context").opened(buffered("context", b"A"), buffered("context", b"B")):
    print(Path("context").read_text())
for path in ["direct", "indirect", "shared", "context"]:
    print(Path(path).read_text())
