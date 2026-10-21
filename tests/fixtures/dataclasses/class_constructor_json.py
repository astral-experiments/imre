from dataclasses import dataclass
from typing import Self

from relar.json import decode as cls


@dataclass
class Item:
    value: int

    @classmethod
    def make(cls, value: int) -> Self:
        return cls(value)


print(Item.make(18).value)
print(cls('{"value": 19}', Item).value)
