"""Observe line terminators, predicate order, and errors through the public functions."""
import textwrap
from textwrap import TextWrapper, dedent, indent


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

# Choosing a word separator uses identity with True; breaking long chunks
# uses truth. Preserve the type even after changing options on the same object.
flags: list[bool | int] = [True, False, 1, 0, -1, 2**100]
wrapper = TextWrapper(width=7, break_long_words=False)
for flag in flags:
    wrapper.break_on_hyphens = flag
    print(repr(wrapper.break_on_hyphens))
    print(repr(wrapper._split("Ärger-über -- foo _²-²_")))
    print(repr(wrapper.wrap("go-go go-go")))
    wrapper.break_long_words = flag
    print(repr(wrapper.wrap("word-too-long")))
wrapper.tabsize = 3
print(repr(wrapper.fill("a\tb\r\nC\u2028D")))
print(repr(textwrap.fill("  déjà-vu   ", width=6)))
print(repr(textwrap.shorten("go \u00a0 west", width=8, placeholder="…")))
