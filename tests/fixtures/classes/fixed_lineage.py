"""A derived fixed layout shares its base's fields and specializes its methods."""

from __future__ import annotations

import dataclasses
from abc import ABC, abstractmethod
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass, field


@dataclass(frozen=True)
class Entry:
    letter: str
    enabled: bool | None = None
    offset: int = field(default=0, compare=False)

    def active(self) -> Entry:
        return Entry(**{**dataclasses.asdict(self), "enabled": True})


class Result:
    def value(self) -> bool | int | None:
        return 17


class Missing(Result):
    def value(self) -> None:
        return None


class FalseResult(Result):
    def value(self) -> bool:
        return False


class ZeroResult(Result):
    def value(self) -> int:
        return 0


def observe_result(result: Result) -> None:
    value = result.value()
    if value is None:
        print("missing")
    elif isinstance(value, bool):
        print("boolean")
        print(value)
    else:
        print("integer")
        print(value)


class Rows(ABC):
    def __init__(self, lines: Iterable[str]) -> None:
        self.lines = list(lines)
        self.added = [line[1:] for line in self.lines if line.startswith("+")]

    def __iter__(self) -> Iterator[str]:
        yield from self.lines

    def __bool__(self) -> bool:
        return bool(self.added)

    @abstractmethod
    def count(self) -> int:
        pass


class MoreRows(Rows):
    def __init__(self, lines: list[str], entries: set[Entry]) -> None:
        self.entries = entries
        super().__init__(lines)

    def count(self) -> int:
        return len(self.entries) + len(self.added)


class Trail:
    marks: str

    def __init__(self) -> None:
        self.marks = ""

    def mark(self, value: str) -> str:
        self.marks = self.marks + value
        return value

    def tag(self) -> str:
        return "base"

    def entries(self, first: str = "base", second: str = "last") -> Iterator[str]:
        self.marks = self.marks + "B"
        yield from [first, second]

    def render(self, first: str, second: str) -> str:
        self.marks = self.marks + "R"
        return first + self.tag() + second


class Middle(Trail):
    def entries(self, first: str = "middle", second: str = "last") -> Iterator[str]:
        self.marks = self.marks + "M"
        yield from [first, second]

    def render(self, first: str, second: str) -> str:
        self.marks = self.marks + "M"
        return super().render(second=self.mark(second), first=self.mark(first))

    def stop(self) -> str:
        self.marks = self.marks + "!"
        raise ValueError("deliberate")

    def interrupted(self) -> str:
        return super().render(second=self.mark("S"), first=self.stop())


class Leaf(Middle):
    def tag(self) -> str:
        return "leaf"

    def through_middle(self) -> str:
        return super().render("x", "y")


def from_base(rows: Rows) -> None:
    count: Callable[[], int] = rows.count
    print(count())


def pull_method(trail: Trail) -> None:
    empty: list[int] = []
    for index, label in zip(empty, trail.entries(second=trail.mark("2"), first=trail.mark("1"))):
        print(label)
    print(trail.marks)
    for label in trail.entries():
        print(label)
        break
    print(trail.marks)


def run() -> None:
    entry = Entry("a")
    other = Entry("a", offset=7)
    entries = {entry, other, entry.active()}
    rows = MoreRows(["+first", "second"], entries)
    print(rows.count())
    from_base(rows)
    print(isinstance(rows, Rows))
    print(entry in rows.entries)
    print(bool(rows))
    for line in rows:
        print(line)
        if line == "+first":
            rows.lines.append("third")
    print(bool(MoreRows([], entries)))
    middle = Middle()
    leaf = Leaf()
    print(middle.render("a", "b") + " " + middle.marks)
    print(leaf.render("a", "b") + " " + leaf.marks)
    print(leaf.through_middle() + " " + leaf.marks)
    pull_method(Trail())
    pull_method(Leaf())
    try:
        leaf.interrupted()
    except ValueError as exc:
        print(str(exc) + " " + leaf.marks)


run()

observe_result(Result())
observe_result(Missing())
observe_result(FalseResult())
observe_result(ZeroResult())
