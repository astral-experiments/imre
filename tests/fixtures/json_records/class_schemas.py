from dataclasses import dataclass
from typing import Self, Union as Nullable

from relar.json import decode


@dataclass
class cls:
    unrelated: str


@dataclass
class Record:
    value: int

    @classmethod
    def parse(cls, text: str) -> Self:
        return decode(text, cls)

    @classmethod
    def rows(cls, text: str) -> list[Self]:
        return decode(text, list[Record])

    @classmethod
    def grouped(cls, text: str) -> dict[str, list[Self | None]]:
        return decode(text, dict[str, list[Nullable[Record, None]]])


print(Record.parse('{"value":4}').value)
print(Record.rows('[{"value":5}]')[0].value)
groups = Record.grouped('{"first":[{"value":6},null]}')
first = groups["first"][0]
if first is not None:
    print(first.value)
print(groups["first"][1] is None)
