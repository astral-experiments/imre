"""Plain dictionaries map ordinals lazily; None deletes, a miss preserves."""
import logging


def ints(text: str, table: dict[int, int]) -> None:
    try: print(repr(text.translate(table)))
    except ValueError as error: print(error)


def names(text: str, table: dict[int, str | None]) -> None:
    print(repr(text.translate(table)))


def mixed(text: str, table: dict[int, int | None]) -> None:
    print(repr(text.translate(table)))


def invalid(text: str, table: dict[int, str | bytes]) -> None:
    try: print(repr(text.translate(table)))
    except TypeError as error: print(error)


def booleans(text: str, table: dict[int, bool | None]) -> None:
    print(repr(text.translate(table)))


def invalid_float(text: str) -> None:
    table: dict[int, float] = {97: 0.5}
    try: print(repr(text.translate(table)))
    except TypeError as error: print(error)


def source(text: str) -> str:
    print("receiver")
    return text


def key(n: int) -> int:
    print("key " + str(n))
    return n


def number(n: int) -> int:
    print("number " + str(n))
    return n


def replacement(text: str) -> str:
    print("replacement " + text)
    return text


def make_table() -> dict[int, int]:
    print("table")
    return {97: 98}


def late() -> int:
    print("late")
    raise ValueError("late argument failed")


def identity(value: str) -> None:
    lookup: dict[int, int] = {}
    print(value.translate(lookup) is value)


# getLevelName returns a closed str-or-int result; the integer is boxed even if small.
def from_level(level: str) -> None:
    table = {97: logging.getLevelName(level)}
    try: print(repr("a?".translate(table)))
    except ValueError as error: print(str(error))


whitespace = {9: 32, 10: 32, 11: 32, 12: 32, 13: 32, 32: 32}
for text in ["", " ", "hello", "x\t\r\n\v\f y", "☼\u2028\x85\xa0"]:
    ints(text, whitespace)
values: dict[int, str | None] = {97: None, 98: "ñ🌍", 99: ""}
names("cccaaabbb\0", values)
values[97] = "<A>"
names("aab", values)
mixed("aaabbc", {97: None, 98: 105})
print(repr("abababc".translate({97: None, 98: 105})))
print(repr("abababc".translate({97: "", 98: None})))
print(repr("a".translate({97: None})))
print(repr("abababc".translate({97: None, 98: 105, 99: "x"})))
print(repr("cba".translate({97: "xy", 98: 0x10ffff})))
print(repr("cba".translate({98: 0x10ffff, 97: "xy"})))
print(repr("aba".translate({key(97): number(100), key(97): replacement("xy"), key(98): None})))
print(repr("b".translate({98: False})))
print(repr("b".translate({98: True})))
print(repr("cab".translate({"a": "z"})))
print(repr("cab".translate({b"a": "z"})))
print(repr("cab".translate({(97, 98): "z"})))
ints("a\0a", {97: 0x10ffff, 0: 128})
for bad in [-1, 0x110000, 1 << 200, -(1 << 200)]:
    ints("", {97: bad})
    ints("zz", {97: bad})
    ints("za", {97: bad})
invalid("a", {97: "valid", 98: b"bad"})
invalid("b", {97: "valid", 98: b"bad"})
booleans("abcdab", {97: False, 98: True, 99: None})
numeric_members: dict[int, bool | int | None] = {97: False, 98: True, 99: None, 100: 65}
print(repr("abcde".translate(numeric_members)))
invalid_float("")
invalid_float("zz")
invalid_float("a")
for text in ["", "z", "zz", "ÿ", "Ā", "界", "🙂"]:
    identity(text)
identity("A".lower())
identity("Ÿ".lower())
identity(chr(255))
print(repr("abc".translate({})))
try: print("x".translate({}, {}))
except TypeError as error: print(error)
print(repr(source("aba").translate(make_table())))
try: print(source("x").translate())
except TypeError as error: print(error)
try: print(source("x").translate(make_table(), make_table()))
except TypeError as error: print(error)
try: print(source("x").translate(table=make_table()))
except TypeError as error: print(error)
try: print(source("x").translate(make_table(), ignored=late()))
except ValueError as error: print(error)


from_level("INFO")
from_level("not_a_registered_level")
logging.addLevelName(0x10ffff, "LAST")
logging.addLevelName(1 << 100, "BIG")
from_level("LAST")
from_level("BIG")
