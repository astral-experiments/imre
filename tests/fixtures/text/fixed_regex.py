import re


MARKER = re.compile(
    r"""
    \# \s* E                 # marker
    (?P<optional>\?)?
    (?:\[(?P<tag>[^+\]]+)(?P<multi>\+)?\])?
    (?:\s*:\s*(?P<description>.*))?
    """,
    re.VERBOSE,
)
WIDE = re.compile(r"(a)(b)(c)(d)(e)(f)(g)(h)(?P<last>x?)")


def capture(value: str | None) -> str:
    if value is None:
        return "<absent>"
    return "[" + value + "]"


def first(pattern: re.Pattern[str], text: str) -> re.Match[str] | None:
    return pattern.search(text)


def retained(prefix: str) -> re.Match[str] | None:
    line = prefix + " # E[tag+]: π"
    found = re.search(MARKER, line)
    line = "replaced"
    return found


def show(found: re.Match[str] | None) -> None:
    if found:
        print(found.group())
        print(found.start())
        print(found.end(0))
        print(capture(found.group(1)))
        print(capture(found.group("tag")))
        print(found.start("tag"))
        print(found.end(2))
        print(capture(found.group("multi")))
        print(capture(found.group("description")))
    else:
        print("no match")


show(retained("é🙂"))
for line in ["# E", "# E:", "# E?:  ", "# E[tag++]", "x # no #\x1cE", "#\u00a0E", "#\x1fE", "#\u200bE", "# E:\nA", ""]:
    show(first(MARKER, line))

for text in ["", "a", "aaaa"]:
    greedy = re.search(r"a{,2}", text)
    lazy = re.search(r"a{,2}?", text)
    unbounded = re.search(r"a{,}", text)
    if greedy and lazy and unbounded:
        print(greedy.group())
        print(lazy.group())
        print(unbounded.group())

# Empty matches are true, including a match at the end of an empty string.
for text in ["ab", ""]:
    if empty := re.search(r"a*?", text):
        print(empty.start())
        print(empty.end())
        print(capture(empty.group(0)))

# Spaces and hashes inside a class stay literal in verbose mode.
if literal := re.compile(r"[ #]+\ \#", re.VERBOSE).search("é#  #"):
    print(literal.group())
    print(literal.start())
    print(literal.end())

if matched := MARKER.search("# E"):
    try:
        matched.group("unknown")
    except IndexError:
        print("unknown name")
    try:
        matched.start(123)
    except IndexError:
        print("unknown number")

# A later successful search must not overwrite an earlier match's captures.
early = WIDE.search("éabcdefgh")
late = WIDE.search("🙂🙂abcdefghx")
if early and late:
    print(early.start())
    print(capture(early.group("last")))
    print(early.end(9))
    print(late.start(1))
    print(capture(late.group(9)))
