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


def matched(name: str | bytes | Path, pat: str | bytes | Path, fn=fnmatch.fnmatch) -> bool:
    return fn(name, pat)


def never_match(name: str | bytes | Path, pat: str | bytes | Path) -> bool:
    return False


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


print(matched("abc", "*", never_match))
print(matched("é", "?"))
print(matched(b"\xc3\xa9", b"??", fnmatch.fnmatchcase))
print(matched(Path("dir/.hidden"), Path("*/.*")))
try:
    print(matched(Path("dir/.hidden"), "*/.*", fnmatch.fnmatchcase))
except TypeError as exc:
    print(str(exc))
try:
    print(matched(b"abc", "*"))
except TypeError as exc:
    print(str(exc))

# Literal affixes may meet but cannot share any bytes of the name.
for candidate_name, star_pattern in [("test", "tes*est"), ("abc", "a***bc"), ("", "***"),
                                     ("é/雪", "é*雪"), ("雪", "雪*雪"), ("aXb", "a?*b")]:
    print(fnmatch.fnmatchcase(candidate_name, star_pattern))
print(fnmatch.fnmatchcase(b"\xff/\x80", b"\xff**\x80"))
