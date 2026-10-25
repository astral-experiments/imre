"""Byte values, subsequences, and evaluation order in bytes containment."""

from collections import Counter, defaultdict


def search(needle: bytes, data: bytes) -> None:
    print(needle in data)
    print(needle not in data)


def integer(needle: int, data: bytes) -> None:
    try:
        print(needle in data)
        print(needle not in data)
    except ValueError as error:
        print(error)
    try:
        print(needle not in data)
    except ValueError as error:
        print(error)


def marked(label: str, value: bytes) -> bytes:
    print(label)
    return value


def wrong() -> str:
    print("wrong needle")
    return "a"


def invalid() -> int:
    print("invalid integer")
    return 256


def failed(label: str) -> bytes:
    print(label)
    raise RuntimeError(label)


def replace_needle(values: list[bytes]) -> bytes:
    values[0] = b"new"
    return b"old"


def keep_needle(data: list[int]) -> None:
    values = [bytes(data)]
    print(values[0] in replace_needle(values))
    print(values[0])


for needle in [b"", b"a", b"b", b"c", b"ab", b"bc", b"abc", b"ac", b"d", b"dab", b"abd"]:
    search(needle, b"abc")
for needle in [b"", b"\x00", b"\xff", b"\xff\x00", b"\x00\xff"]:
    search(needle, b"")
    search(needle, b"\xff\x00\xff")
for value in [0, 1, 97, 200, 255, 256, 300, -1, 9223372036854775808, 1 << 100, -(1 << 100)]:
    integer(value, b"\x00abc\xff")
    integer(value, b"")
for truth in [False, True]:
    print(truth in b"\x00")
    print(truth not in b"\x01")
    print(truth in b"")
    print(truth in b"\x00\x01")

try:
    print(None in b"")
except TypeError as error:
    print(error)
try:
    print(97.0 in b"abc")
except TypeError as error:
    print(error)
try:
    print(wrong() not in marked("right before TypeError", b"abc"))
except TypeError as error:
    print(error)
try:
    print([97] in b"abc")
except TypeError as error:
    print(error)
try:
    print((97,) in b"abc")
except TypeError as error:
    print(error)
try:
    print({"a": 97} in b"abc")
except TypeError as error:
    print(error)
try:
    print({97} in b"abc")
except TypeError as error:
    print(error)

print(marked("left", b"ac") not in marked("right", b"abc"))
try:
    print(failed("left failed") in marked("unreachable right", b"abc"))
except RuntimeError as error:
    print(error)
try:
    print(invalid() in failed("right before ValueError"))
except RuntimeError as error:
    print(error)
try:
    print(wrong() not in failed("right before TypeError"))
except RuntimeError as error:
    print(error)

try:
    print(Counter([1]) in b"")
except TypeError as error:
    print(error)
try:
    values: defaultdict[str, list[int]] = defaultdict(list)
    print(values in b"")
except TypeError as error:
    print(error)


keep_needle([111, 108, 100])
