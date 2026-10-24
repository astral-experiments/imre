"""A suspended call transfers buffered handles to its parameters on entry."""

from __future__ import annotations

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


@dataclass(repr=False)
class Holder:
    stream: BinaryIO

    async def replace(self, other: Holder, path: str) -> None:
        self = other
        print(Path(path).read_text())
        self.stream.write(b"C")
        if path == "failure":
            raise ValueError(path)


def later(
    callback: Callable[[Holder, str], Awaitable[None]], other: Holder, path: str
) -> Awaitable[None]:
    return callback(other, path)


async def exercise() -> None:
    await rebind(buffered("direct", b"A"), buffered("direct", b"B"), "direct")
    callback: Callable[[BinaryIO, BinaryIO, str], Awaitable[None]] = rebind
    await callback(buffered("indirect", b"A"), buffered("indirect", b"B"), "indirect")
    shared = buffered("shared", b"A")
    await callback(shared, shared, "shared")
    shared.write(b"D")
    shared.close()
    await later(
        Holder(buffered("temporary", b"A")).replace,
        Holder(buffered("temporary", b"B")),
        "temporary",
    )
    retained = Holder(buffered("retained", b"A")).replace
    await later(retained, Holder(buffered("retained", b"B")), "retained")
    await later(retained, Holder(buffered("retained", b"D")), "retained")
    try:
        await later(
            Holder(buffered("failure", b"A")).replace,
            Holder(buffered("failure", b"B")),
            "failure",
        )
    except ValueError as error:
        print(str(error))


asyncio.run(exercise())
with Output("context").opened(buffered("context", b"A"), buffered("context", b"B")):
    print(Path("context").read_text())
for path in ["direct", "indirect", "shared", "context", "temporary", "retained", "failure"]:
    print(Path(path).read_text())
