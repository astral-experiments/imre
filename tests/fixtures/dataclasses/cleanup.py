from dataclasses import dataclass, field
from typing import Self


@dataclass(slots=True)
class Owned:
    name: str
    values: list[str] = field(default_factory=list)

    @classmethod
    def make(cls, index: int) -> Self:
        if index == 3:
            raise ValueError("stop construction")
        return cls("long owned string beyond compact storage " + str(index))

    @property
    def first(self) -> str:
        return self.values[0]


for index in range(8):
    try:
        item = Owned.make(index)
        item.values.append(item.name)
        print(item.first)
    except ValueError as error:
        print(str(error))
try:
    print(Owned("missing").first)
except IndexError:
    print("missing first")
