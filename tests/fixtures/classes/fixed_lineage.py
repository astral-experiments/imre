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

    def render(self, first: str, second: str) -> str:
        self.marks = self.marks + "R"
        return first + self.tag() + second


class Middle(Trail):
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
    try:
        leaf.interrupted()
    except ValueError as exc:
        print(str(exc) + " " + leaf.marks)


run()
