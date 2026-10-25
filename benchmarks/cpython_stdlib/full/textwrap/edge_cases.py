"""Observe line terminators, predicate order, and errors through the public functions."""
from textwrap import dedent, indent


def visible(line: str) -> bool:
    print("visit " + repr(line))
    if line.startswith("stop"):
        raise LookupError("stop here")
    return not line.isspace()


def number() -> int:
    print("evaluate number")
    return 42


def fail() -> int:
    raise ValueError("argument error")


for text in ("", " ", "a\r\nb\rc\nd", "\u001c\u001d\u001e\u001f", "\v\f\x85\u2028\u2029",
             "a\n\u00a0\n🙂\x00tail\r\n", "ok\nstop\nnever\n"):
    print(repr(indent(text, "→\n")))
    try:
        print(repr(indent(text, "", predicate=visible)))
    except LookupError as exc:
        print(str(exc))

try:
    dedent(text=number())
except TypeError as exc:
    print(str(exc))
try:
    dedent(fail())
except ValueError as exc:
    print(str(exc))
