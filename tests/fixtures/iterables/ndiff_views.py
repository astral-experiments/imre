from dataclasses import dataclass
from difflib import ndiff


@dataclass
class Lines:
    rows: list[str]


def signed(rows: list[str]) -> list[str]:
    return [line for line in rows if line.startswith(("+", "-"))]


def added(rows: list[str], spaces: int) -> list[str]:
    return [
        line[2:]
        for line in rows
        if line.startswith("+" + " " * spaces) and not line.startswith("+++")
    ]


def removed(rows: list[str], spaces: int) -> list[str]:
    return [
        line[2:]
        for line in rows
        if line.startswith("-" + " " * spaces) and not line.startswith("---")
    ]


def show(left: list[str], right: list[str]) -> None:
    rows = list(ndiff(left, right))
    changes = signed(rows)
    print(repr(sorted(changes, key=lambda line: line[2:])))
    print(repr(added(changes, 1)))
    print(repr(removed(rows, 1)))
    print(repr(changes))
    # Raw sorting includes unchanged lines and intraline guides.
    print(repr(sorted(rows, key=lambda line: line[2:])))
    print(repr(rows))


def key(line: str) -> str:
    print(repr(line))
    return line[2:]


def prefix(line: str, seen: list[str]) -> str:
    seen.append(line)
    return "+"


def snapshots() -> None:
    left = ["café abcdef\n", "same"]
    right = ["café abcXef\n", "same"]
    rows = list(ndiff(left, right))
    # list(ndiff(...)) observes its inputs now, even if no one reads it yet.
    left[0] = "after"
    right.pop()
    changes = signed(rows)
    copy = list(rows)
    print(Lines(rows) == Lines(copy))
    alias = rows
    alias[0] = "edited"
    print(repr(changes))
    print(repr(copy))
    print(repr(rows))
    print(Lines(copy) == Lines(rows))
    print(Lines(alias) == Lines(rows))
    copy.append("independent")
    print(len(copy))
    print(len(alias))
    seen: list[str] = []
    print(repr([line[2:] for line in copy if line.startswith(prefix(line, seen))]))
    print(repr(seen))
    print(repr(sorted(rows, key=key)))


def repeat_errors(rows: list[str]) -> None:
    try:
        print(repr(added(rows, 99999999999999999999999999)))
    except OverflowError:
        print("overflow")


def indexed() -> None:
    rows = list(ndiff(["same"], ["same"]))
    print("  same" in rows)
    for i in range(len(rows)):
        print(rows[i])
    print(rows[-1])
    print(repr(Lines(signed(list(ndiff(["a"], ["b"]))))))


def sparse_snapshots() -> None:
    left = ["", "", "café", "cafê"]
    right = list(left)
    for i in range(10000, 11100):
        text = f"row {i} 雪\x00"
        left.append(text)
        right.append(text)
    left.extend(["tail abcdefgh", "tail duplicate", "tail duplicate"])
    right.extend(["tail abcdEfgh", "tail duplicate"])
    rows = list(ndiff(left, right))
    copy = list(rows)
    changes = signed(rows)
    print(repr(sorted(changes, key=lambda line: line[2:])))
    left[-1] = "edited source"
    right.pop()
    print(repr(added(copy, 1)))
    print(repr(copy))
    alias = rows
    alias[0] = "edited alias"
    print(Lines(copy) == Lines(rows))
    print(repr(copy[:4]))
    print(repr(rows[:4]))


def select_prefix(rows: list[str], start: str, width: int) -> list[str]:
    return [row[1:] for row in rows if row.startswith(start + "x" * (width + 1))]


# Both signed subsequences preserve their own source order, including
# identical payloads, and callers can also observe the order of their signs.
show(["a", "a", "b", "café"], ["a", "b", "b", "雪"])
show(["z", "a", "z", "x"], ["a", "z", "x", "z"])
show(["a", "a", "b"], ["b", "a", "a"])
show(["abcdefg", "common"], ["abcXefg", "common"])
show([], [])
snapshots()
indexed()
sparse_snapshots()

# These strings are ordinary list elements. Prefixes alone do not imply
# that the rows came from ndiff.
plain = [
    "+++ header",
    "--- header",
    "+ ",
    "- ",
    "+",
    "-x",
    "++",
    "--",
    "+ 雪",
    "  same",
]
for spaces in [-1, 0, 1, 2]:
    print(repr(added(plain, spaces)))
    print(repr(removed(plain, spaces)))
unicode_rows = ["雪🦀z", "éx", "a", "雪🦀a", "éxa"]
print(repr([line[2:] for line in unicode_rows if line.startswith(("雪", "é"))]))
print(repr(sorted(unicode_rows, key=lambda line: line[2:])))
repeat_errors(list(ndiff([], [])))
repeat_errors(list(ndiff(["unchanged"], ["unchanged"])))
repeat_errors(signed(list(ndiff(["unchanged"], ["unchanged"]))))
print(repr(select_prefix(["éxx", "éxxx!", "z", "éx"], "é", 1)))
