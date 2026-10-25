from dataclasses import dataclass, field
from typing import Callable, Self

import class_alias_helpers as cls
from class_alias_helpers import identity as __imre_parameter_0
from class_alias_helpers import key as __imre_parameter_2


@dataclass
class Item:
    value: int

    @staticmethod
    def identity(value: int) -> int:
        return value + 1

    @staticmethod
    def key(value: int) -> int:
        return value

    @staticmethod
    def positive(value: int) -> bool:
        return value > 0

    @classmethod
    def make(cls, value: int) -> Self:
        return cls(value)

    @classmethod
    def items(cls) -> list[int]:
        return [cls(2).value, cls(3).value]

    @classmethod
    def exercise(cls) -> None:
        # Every path must bind cls to this class before resolving module names.
        print(cls.identity(4))
        expected: Callable[[int], int] = cls.identity
        inferred = cls.identity
        print(expected(5))
        print(inferred(6))
        print(sum(map(cls.identity, [5, 6])))
        print(sum(filter(cls.positive, [-1, 0, 1])))
        print(sorted([3, 1, 2], key=cls.key)[0])
        print(sum(cls.identity(value) for value in [7, 8]))
        expanded = [*cls.items()]
        print(sum(expanded))
        print(sum(value for value in cls.items()))
        deferred: Callable[[int], int] = lambda value: cls.identity(value)
        print(deferred(9))
        print(cls.make(11).value)


@dataclass(kw_only=True)
class Names:
    cls: int
    len: int
    values: list[int] = field(default_factory=list)


Item.exercise()
names = Names(cls=12, len=13)
print(names)
print(__imre_parameter_0(names.cls))
print(__imre_parameter_2(names.len))
print(cls.identity(14))
