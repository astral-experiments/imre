from dataclasses import dataclass
from typing import Callable, Self


@dataclass(slots=True)
class Item:
    value: int

    @classmethod
    def make(cls, value: int, *, extra: int = 2) -> Self:
        return cls(value + extra)

    @classmethod
    def from_value(cls, value: int) -> Self:
        return cls(value)

    @staticmethod
    def increment(value: int, amount: int = 1) -> int:
        return value + amount

    def same(self) -> Self:
        return self

    def plus(self, amount: int) -> int:
        return self.value + amount

    @property
    def operation(self) -> Callable[[int], int]:
        return self.plus


@dataclass
class ClassItems:
    @classmethod
    def items(cls) -> list[int]:
        return [2, 3]


@dataclass
class StaticItems:
    @staticmethod
    def items() -> list[int]:
        return [5, 7]


def receiver() -> Item:
    print("receiver")
    return Item(100)


def argument() -> int:
    print("argument")
    return 3


print(Item.make(5).value)
print(receiver().make(argument(), extra=4).value)
print(receiver().increment(argument(), amount=2))
make: Callable[[int], Item] = receiver().from_value
increment: Callable[[int, int], int] = receiver().increment
print(make(8).value)
print(increment(10, 3))
item = Item(4)
alias = item.same()
alias.value = 6
print(item.value)
operation: Callable[[int], int] = receiver().operation
print(operation(5))
print(sum(ClassItems.items()))
print(sum(list(StaticItems.items())))
print(sum([*ClassItems().items()]))
print(sum(value for value in StaticItems().items()))
items: Callable[[], list[int]] = StaticItems.items
print(sum(items()))
