"""Differential for public credential converters; run only in a disposable child."""

import logging
import os
import posix


class Index:
    def __index__(self) -> int:
        print("index")
        return -1


class BadIndex:
    def __index__(self) -> str:
        print("bad index")
        return "bad"


class Raises:
    def __index__(self) -> int:
        print("index raised")
        raise ValueError("from __index__")


class NullableIndex:
    value: int | None

    def __init__(self, value: int | None) -> None:
        self.value = value

    def __index__(self) -> int | None:
        return self.value


class LevelIndex:
    name: str

    def __init__(self, name: str) -> None:
        self.name = name

    def __index__(self):
        return logging.getLevelName(self.name)


class ReturnsIndex:
    def __index__(self) -> Index:
        return Index()


class Derived(Index):
    def __index__(self) -> int:
        print("derived index")
        return -2


def as_base(value: Index) -> None:
    os.setreuid(value, -1)


def optional(value: int | None) -> None:
    posix.setregid(value, -1)


def level_argument(name: str) -> None:
    # getLevelName has a native int-or-str result. The invalid second argument
    # ensures neither variant can reach a credential-changing syscall.
    try:
        posix.setregid(logging.getLevelName(name), None)
    except TypeError as exc:
        print(str(exc))
    else:
        raise AssertionError("union gid")


def nullable_result(value: int | None) -> None:
    try:
        os.setregid(NullableIndex(value), None)
    except TypeError as exc:
        print(str(exc))
    except OverflowError as exc:
        print(str(exc))


def argument() -> int:
    print("second argument")
    return -1


assert os.getuid() == posix.getuid() and os.geteuid() == posix.geteuid()
assert os.getgid() == posix.getgid() and os.getegid() == posix.getegid()
assert os.setreuid(Index(), argument()) is None
assert posix.setregid(-1, -1) is None
as_base(Index())
try:
    as_base(Derived())
except OverflowError as exc:
    print(str(exc))
else:
    raise AssertionError("subclass index")
optional(-1)
try:
    optional(None)
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("nullable gid")
level_argument("WARNING")
level_argument("imre-unknown-level")

nullable_result(-1)
nullable_result(-2)
nullable_result(None)
for name in ["WARNING", "imre-unknown-level"]:
    try:
        os.setregid(LevelIndex(name), None)
    except TypeError as exc:
        print(str(exc))
try:
    os.setregid(ReturnsIndex(), None)
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("nested index result")

for value in [-2, 1 << 32, 1 << 62, 1 << 63, 1 << 100, -(1 << 100)]:
    try:
        os.setreuid(value, -1)
    except OverflowError as exc:
        print(str(exc))
    else:
        raise AssertionError("uid converter accepted an out-of-range input")
    try:
        posix.setregid(-1, value)
    except OverflowError as exc:
        print(str(exc))
    else:
        raise AssertionError("gid converter accepted an out-of-range input")

try:
    posix.setreuid(-2, "bad")
except OverflowError as exc:
    print(str(exc))
else:
    raise AssertionError("conversion order")
try:
    os.setregid(False, None)
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("invalid gid")
try:
    os.setreuid(BadIndex(), argument())
except TypeError as exc:
    print(str(exc))
else:
    raise AssertionError("invalid index result")
try:
    os.setreuid(Raises(), argument())
except TypeError as exc:
    assert exc.__cause__ is None and not exc.__suppress_context__
    print(str(exc))
else:
    raise AssertionError("exception in __index__")

try:
    posix.getuid(argument())
except TypeError as exc:
    print(str(exc))
try:
    os.setgid(gid=argument())
except TypeError as exc:
    print(str(exc))
try:
    os.setreuid(-1)
except TypeError as exc:
    print(str(exc))
