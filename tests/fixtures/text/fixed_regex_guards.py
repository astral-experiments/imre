import re


FIRST = re.compile(r"(?P<tag>a)(?P<empty>b?)(?P<optional>z)?")
SECOND = re.compile(r"(?P<other>x)(?P<tag>y)")


def stable(text: str) -> None:
    if found := FIRST.search(text):
        print(found.group("tag") or "missing")
        print(found.group("empty") is None)
        print(found.group("optional") is not None)
        print(found.start(1))
        print(found.end("empty"))
        try:
            print(found.group("other") is None)
        except IndexError:
            print("bad name")
        try:
            print(found.start(4))
        except IndexError:
            print("bad index")


def replace(change: bool) -> None:
    if found := FIRST.search("ab"):
        if change:
            found = SECOND.search("xy")
        if found:
            print(found.group("tag") or "missing")
            try:
                print(found.group("empty") is None)
            except IndexError:
                print("other pattern")


stable("πa")
stable("abz")
replace(False)
replace(True)
