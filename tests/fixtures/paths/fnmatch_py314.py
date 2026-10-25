from collections.abc import Iterator
import fnmatch
from fnmatch import filter
from pathlib import Path


def names() -> tuple[str, str]:
    print("names")
    return "a/b", "b"


def pattern() -> str:
    print("pattern")
    return "a*"


def source() -> Iterator[str]:
    for name in ["a/b", "b"]:
        print("yield " + name)
        yield name


def mixed_source(entries: list[str | bytes]) -> Iterator[str | bytes]:
    for name in entries:
        print("visit " + repr(name))
        yield name


def mixed_filters(entries: list[str | bytes]) -> None:
    try:
        print(repr(filter((name for name in entries), "a*")))
    except TypeError as exc:
        print(str(exc))
    try:
        print(repr(fnmatch.filterfalse(mixed_source(entries), b"a*")))
    except TypeError as exc:
        print(str(exc))


print(repr(filter(pat=pattern(), names=names())))
print(repr(fnmatch.filterfalse(names(), pattern())))
print(repr(list(filter(["a", "b"], "a"))))
print(repr(fnmatch.filter("πa💙", "?")))
print(repr(fnmatch.filterfalse(source(), "a*")))
try:
    print(repr(filter(source(), b"*")))
except TypeError as exc:
    print(str(exc))
try:
    print(repr(filter(["before", b"bad", "after"], "*")))
except TypeError as exc:
    print(str(exc))
print(fnmatch.fnmatchcase(b"\xc3\xa9", b"??"))
print(fnmatch.fnmatchcase("é", "?"))
print(fnmatch.fnmatch("dir/.hidden", "*/.*"))
print(fnmatch.fnmatch(Path("dir/.hidden"), Path("*/.*")))
print(repr(fnmatch.filter(["dir/.hidden"], Path("*/.*"))))
print(repr(fnmatch.translate(b"")))
try:
    print(fnmatch.translate(b"x"))
except TypeError as exc:
    print(str(exc))
print(repr(fnmatch.translate("**[b-a]*[!b-a]")))
mixed_filters(["ab", "z"])
mixed_filters([b"ab", b"z"])
mixed_filters([b"z", "mixed", b"unread"])
