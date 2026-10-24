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


run()
