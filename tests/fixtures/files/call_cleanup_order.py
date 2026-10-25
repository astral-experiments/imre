"""Buffered writes expose the last owner at a call, a rebind, and a dictionary update."""

from pathlib import Path
from typing import BinaryIO, Callable


def buffered(path: str, data: bytes) -> BinaryIO:
    stream = open(path, "ab")
    stream.write(data)
    return stream


def show(path: str) -> None:
    print(Path(path).read_text(encoding="utf-8"))


def sink(left: BinaryIO, right: BinaryIO) -> None:
    pass


def rebind(left: BinaryIO, right: BinaryIO, path: str) -> None:
    left = right
    show(path)


def call(callback: Callable[[BinaryIO, BinaryIO, str], None], path: str) -> None:
    callback(buffered(path, b"A"), buffered(path, b"B"), path)


def rebind_list(left: list[BinaryIO], right: list[BinaryIO], path: str) -> None:
    left = right
    show(path)


def keep(left: BinaryIO, right: BinaryIO) -> BinaryIO:
    return left


def fail() -> BinaryIO:
    raise ValueError("argument")


def callee_error(left: BinaryIO, right: BinaryIO) -> None:
    raise ValueError("callee")


class Base:
    flag: int

    def __init__(self) -> None:
        self.flag = 1


class Holder(Base):
    stream: BinaryIO

    def __init__(self, stream: BinaryIO) -> None:
        super().__init__()
        self.stream = stream

    def consume(self, other: BinaryIO) -> None:
        pass


class Ignore:
    flag: int

    def __init__(self, left: BinaryIO, right: BinaryIO) -> None:
        self.flag = 1


def invoke(callback: Callable[[BinaryIO], None], path: str) -> None:
    callback(buffered(path, b"B"))


def ignore_one(other: BinaryIO) -> None:
    pass


def rebind_base(left: Base, right: Base) -> None:
    left = right
    show("base")


def mapping(path: str) -> dict[int, BinaryIO]:
    return {0: buffered(path, b"A"), 1: buffered(path, b"B")}


def key(path: str) -> str:
    return "key-" + path


def tuple_mapping(path: str) -> dict[tuple[str, str], BinaryIO]:
    return {
        (key(path), key(path)): buffered(path, b"A"),
        ("x", "y"): buffered(path, b"B"),
    }


def local_frame() -> None:
    first = buffered("locals", b"A")
    second = buffered("locals", b"B")


local_frame()
sink(buffered("positional", b"A"), buffered("positional", b"B"))
sink(right=buffered("keywords", b"A"), left=buffered("keywords", b"B"))
call(rebind, "rebind")
rebind_list([buffered("nested", b"A")], [buffered("nested", b"B")], "nested")
keep(buffered("returned", b"A"), buffered("returned", b"B")).write(b"C")
invoke(Holder(buffered("bound", b"A")).consume, "bound")
rebind_base(Holder(buffered("base", b"A")), Holder(buffered("base", b"B")))
invoke(ignore_one, "plain")
Ignore(right=buffered("constructor", b"A"), left=buffered("constructor", b"B"))
shared = buffered("shared", b"A")
sink(shared, shared)
shared.write(b"B")
shared.close()
try:
    sink(buffered("arg-error", b"A"), fail())
except ValueError as error:
    print(str(error))
try:
    callee_error(buffered("callee-error", b"A"), buffered("callee-error", b"B"))
except ValueError as error:
    print(str(error))
mapping("hit").get(0, buffered("hit", b"C"))
mapping("miss").get(2, buffered("miss", b"C"))
mapping("replaced")[0] = buffered("replaced", b"C")
tuple_mapping("tuple-key").get(
    (key("tuple-key"), key("tuple-key")), buffered("tuple-key", b"C")
)
