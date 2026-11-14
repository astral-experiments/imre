from dataclasses import dataclass, field
def values() -> list[int]:
    return [1]
@dataclass
class Item:
    values: list[int] = field(default_factory=values)


first = Item()
second = Item()
supplied = [3]
first.values.append(2)
print(first.values, second.values)
print(first.values is second.values, Item(supplied).values is supplied)


def left() -> list[int]:
    print("left")
    return values()


def right() -> list[int]:
    print("right")
    return values()


@dataclass
class Pair:
    a: list[int] = field(default_factory=left)
    b: list[int] = field(default_factory=right)


print("both")
Pair()
print("supplied")
Pair(supplied)
