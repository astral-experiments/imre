from selectors import SelectorKey
from typing import NamedTuple


class Entry(NamedTuple):
    number: int
    label: str


class Child(Entry):
    pass


class Empty(NamedTuple):
    pass


def current():
    row = Entry(label="eight", number=7)
    key = SelectorKey("fd", 3, 1, row)
    print(repr(row), str(row), [key, row])
    print(Empty(), Child(1, "child"))
    Entry.__annotations__ = {"later": int}
    print(row, Entry(2, "two"), tuple(row))
    cycle = []
    recursive = SelectorKey(1, 1, 1, cycle)
    cycle.append(recursive)
    print(recursive, cycle)

    deep = None
    for _ in range(4000):
        deep = Entry(deep, "link")
    for render in (repr, str):
        try:
            render(deep)
        except RecursionError as error:
            print(type(error).__name__)
        else:
            raise AssertionError("deep NamedTuple rendered past the Python recursion limit")
