from dataclasses import dataclass
from io import UnsupportedOperation as cls
from typing import Self


@dataclass
class Item:
    value: int

    @classmethod
    def make(cls, value: int) -> Self:
        return cls(value)


print(Item.make(20).value)
try:
    raise cls("module alias")
except cls as error:
    print(str(error))
