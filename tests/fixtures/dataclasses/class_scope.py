from dataclasses import dataclass
from typing import Callable, Self


@dataclass
class Item:
    value: int

    @classmethod
    def make(cls: "type[Self]", value: int) -> Self:
        return cls(value)

    @classmethod
    def scopes(cls) -> int:
        eager = [cls for cls in [1, 2]]
        inline = [cls for cls in reversed([3, 4])]
        generated = sum(cls for cls in [5, 6])
        seen = 0
        assigned = [(seen := cls) for cls in [7, 8]]
        local: Callable[[], int] = lambda: (cls := 9)
        parameter: Callable[[int], int] = lambda cls: cls + 10
        constructor: Callable[[int], Item] = lambda value: cls(value)
        nested = [cls(sum(cls for cls in [1, 2])).value for _ in range(2)]
        after: Self = cls(11)
        return (
            sum(eager)
            + sum(inline)
            + generated
            + sum(assigned)
            + seen
            + local()
            + parameter(2)
            + constructor(13).value
            + sum(nested)
            + after.value
        )

    @classmethod
    def leftmost(cls) -> int:
        # The first iterable belongs to the enclosing scope, before its target
        # shadows cls. Later calls must see the original class again.
        values = [cls for cls in [cls(14).value]]
        lazy = sum(cls for cls in [cls(15).value])
        return sum(values) + lazy + cls(16).value

    @classmethod
    def items(cls) -> list[int]:
        return [cls(18).value, cls(19).value]

    @classmethod
    def item_scopes(cls) -> int:
        total = 0
        for value in cls.items():
            total += value
        values = [value for value in cls.items()]
        return total + sum(values) + sum(cls.items())


print(Item.make(17).value)
print(Item.scopes())
print(Item.leftmost())
print(Item.item_scopes())
for value in Item.items():
    print(value)
