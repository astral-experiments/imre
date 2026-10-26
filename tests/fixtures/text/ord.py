"""Ordinal conversions preserve exact Python errors and pull timing."""
from collections.abc import Iterator
from enum import StrEnum

class Letter(StrEnum):
    A = "A"
    Wide = "☀"
    Two = "xx"


class Thing:
    pass


def show(text: str | bytes | None) -> None:
    try:
        print(ord(text))
    except TypeError as error:
        print(str(error))


def mark(n: int) -> str:
    print(n)
    return "x"


def fail() -> str:
    print("fail")
    raise ValueError("argument failed")


def modify(parts: list[str]) -> int:
    print("modify before first pull")
    parts[0] = "Z"
    parts.append("B")
    return 10


def letters(parts: list[str]) -> Iterator[str]:
    for part in parts:
        print("pull: " + part)
        yield part


for s in ["", "A", "\U0010ffff", "é", "á", "👨‍👧", "ab", "a" * 100000]:
    show(s)
for b in [b"", b"\x00", b"\xff", b"zz"]:
    show(b)
show(None)
try: print(ord(42))
except TypeError as error: print(str(error))
try: print(ord(Thing()))
except TypeError as error: print(str(error))
try: print(ord(None))
except TypeError as error: print(str(error))
try: print(ord(False))
except TypeError as error: print(str(error))
try: print(ord(1.5))
except TypeError as error: print(str(error))
try: print(ord([1]))
except TypeError as error: print(str(error))
try: print(ord({"a": "x"}))
except TypeError as error: print(str(error))
try: print(ord((1, 2)))
except TypeError as error: print(str(error))
try: print(ord())
except TypeError as error: print(str(error))
try: print(ord(mark(1), mark(2)))
except TypeError as error: print(str(error))
try: print(ord(character=mark(3)))
except TypeError as error: print(str(error))
try: print(ord(c=mark(4)))
except TypeError as error: print(str(error))
try: print(ord(mark(5), extra=fail()))
except ValueError as error: print(str(error))

_whitespace = '\t\n\x0b\x0c\r '
print(list(map(ord, _whitespace)))
print(list(map(ord, "ñ🙂a")))
print(list(map(ord, [b"A", b"\xff"])))
print(sum(map(ord, ["A", "B"])))
parts = ["A"]
print(sum(map(ord, parts), modify(parts)))
for text in ["x", "ax", ""]:
    try:
        print(list(map(ord, ["a", text, "λ"])))
    except TypeError as error:
        print(str(error))

for value in map(ord, letters(["A", "bad", "X"])):
    print(value)
    break

try:
    for value in map(ord, letters(["A", "bad", "X"])):
        print(value)
except TypeError as e:
    print(str(e))

try: print(list(map(ord, b"AB")))
except TypeError as error: print(str(error))

for letter in [Letter.A, Letter.Wide, Letter.Two]:
    try: print(ord(letter))
    except TypeError as error: print(str(error))
