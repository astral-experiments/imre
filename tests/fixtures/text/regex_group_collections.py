import re


DIAGNOSTIC = re.compile(r"^(?P<diff>[+-])? ?(?P<where>.*?): (?P<code>[A-Z]\d+)(?P<fix> \[\*\])? (?P<message>.*)$")
LOCATION = re.compile(r"^(?P<pre>[+-]) (?P<inner>(?P<path>[^:]+):(?P<lnum>\d+):\d+:) (?P<post>.*)$")


def diagnostic(text: str) -> None:
    found = DIAGNOSTIC.match(text)
    if found is None:
        print("missing")
        return
    values = found.groupdict()
    where: str = values["where"]
    code: str = values["code"]
    message: str = values["message"]
    print(repr((where, code, message, values.get("diff") == "+", values.get("fix") is not None)))
    print(repr((found.group(0), found.end())))


def location(text: str) -> None:
    match = LOCATION.match(text)
    if match is None:
        print("no location")
        return
    pre, inner, path, lnum, post = match.groups()
    print(repr((pre, inner, path, int(lnum), post)))


def mutation(text: str) -> None:
    if found := DIAGNOSTIC.match(text):
        groups = found.groupdict()
        groups["code"] = None
        print(repr((groups["code"] is None, "relar.eol" in groups)))
        print(repr((found.group("code"), found.groups())))


def locally_compiled(text: str) -> None:
    pattern = re.compile(r"(?P<left>[a-z]+)?:(?P<right>[a-z]+)?$")
    if found := pattern.match(text):
        left, right = found.groups()
        print(repr((left == right, left != right, "same" == left, right != "same")))
    if simple := re.search(r"^no groups$", text):
        print(repr(simple.groups()))
        print(repr(simple.groupdict()))


diagnostic("+ α: F١٠ [*] message\n")
diagnostic("β: F42 ")
diagnostic("prefix\nβ: F42 message")
location("- path.py:10:2: utf8 β\n")
location("\n+ path:1:1: x")
mutation("+ α: F10 message")

if found := re.search(r"(?:(?P<a>a)|(?P<b>b))(?P<empty>x*)", "b"):
    print(repr((found.groups(), found.groupdict())))

locally_compiled(":")
locally_compiled("same:same")
locally_compiled("same:")
locally_compiled(":other")
locally_compiled("no groups")
