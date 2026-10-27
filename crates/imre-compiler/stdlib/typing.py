"""Stream annotation classes for the runtime typing module."""

from typing import Generic, TypeVar


AnyStr = TypeVar("AnyStr", bytes, str)


class IO(Generic[AnyStr]):
    __slots__ = ()


class BinaryIO(IO[bytes]):
    __slots__ = ()


class TextIO(IO[str]):
    __slots__ = ()
