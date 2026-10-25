"""Supplemental Template cases; these output comparisons do not add upstream IDs."""
from string import Template
from collections.abc import Mapping
import re


class Order:
    value: str

    def __init__(self, key: str, **kws: str):
        self.value = kws["early"] + key + kws["late"]


def mapping_state(m: Mapping[str, object]) -> None:
    print(bool(m))
    print(len(m))
    try:
        print(str(m["first"]))
    except KeyError as exc:
        print(repr(exc))


def marked(value: str) -> str:
    print(value)
    return value


def order(head: str, /, main: str, **items: str) -> None:
    print(head)
    print(main)
    for key in items:
        print(key)
        print(items[key])


def trailing_match() -> None:
    pattern = re.compile(r"(?P<word>[a-z]+)$")
    for text in ["a\n","č a a","a\n\n","𐐨 ab"]:
        for m in pattern.finditer(text):
            print(m.group(0))
            print(m.start())
            print(m.end())


def independent_matches() -> None:
    word = re.compile(r"(?P<word>[a-z]+)")
    saved: list[re.Match[str]] = []
    for outer in word.finditer("𐐨 abc  def"):
        for inner in word.finditer("x  yz"):
            print(outer.start())
            print(inner.end())
        saved.append(outer)
    for kept in saved:
        print(kept.group(0))
        print(kept.end())


order(marked("pos"), before=marked("before"), main=marked("main"),
      head=marked("shadow"), after=marked("after"))
for source in ["", "$", "$$", "$ok", "${ok}", "$ok$x $ok $$ ${z}", "$K $ı $İ",
               "π\r\nϿ$$\u2028\t$", "A\x1c\x1d$?tail", "$missing $", "$ $missing",
               "x\0$ok", "𐐨\n\n$ ", "end$\n", "${bad-name}"]:
    template = Template(source)
    print(template.is_valid())
    print(repr(template.get_identifiers()))
    try:
        print(repr(template.substitute(ok="π", x="χ", z="Z")))
    except KeyError as exc:
        print(repr(exc))
    except ValueError as exc:
        print(repr(exc))
    print(repr(template.safe_substitute(ok="π", x="χ", z="Z")))

t = Template("$x: $y")
print(t.substitute({"x": "old"}, x=None, y=("π", 4)))
t.template = "${x} $$"
print(t.substitute({"x": (9, 1 << 130)}))
print(t.substitute(x=b"\xfd"))
print(t.substitute(x=False))
print(t.substitute(None, x="override"))
try:
    print(t.safe_substitute(None))
except TypeError as exc:
    print(repr(exc))


trailing_match()
independent_matches()

large = 1 << 15000
print(Template("unused").substitute(dict(x=large)))
for fmt in ["$x", "${x}"]:
    try:
        print(Template(fmt).substitute(x=("nested", large)))
    except ValueError as exc:
        print(str(exc))
mixed = dict(x=42, text="x", zero=None)
print(Template("$x $text $zero").substitute(mixed))
print(repr(mixed["text"]))

limit = 1
for i in range(4300):
    limit *= 10
print(len(Template("$n").substitute(n=limit-1)))
for n in [limit, -limit]:
    try:
        print(len(Template("$n").substitute(n=n)))
    except ValueError as exc:
        print(str(exc))

mapping_state({})
mapping_state({"first": None})
mapping_state({"other": 123})

print(Order(early=marked("constructor early"), key=marked("constructor key"),
            late=marked("constructor late")).value)

# Exercise many separated captures without printing the growing output.
for prefix in ["a", "é𐐨"]:
    dense = Template((prefix + "$k|${k}|$$") * 2048).substitute(k="Ω")
    print(len(dense))
    print(dense[:5])
    print(dense[-5:])

# CPython returns the original object when substitution leaves one whole match
# or literal untouched; constructing an empty result must retain the input too.
for original in ["", "é", "$missing", "keep $missing", "$$"]:
    print(Template(original).safe_substitute() is original)
for original in ["", "é"]:
    print(Template(original).substitute() is original)
