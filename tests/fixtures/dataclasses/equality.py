from dataclasses import dataclass, field


@dataclass
class Leaf:
    name: str
    number: int


@dataclass
class OtherLeaf:
    name: str
    number: int


@dataclass(slots=True)
class Record:
    leaf: Leaf
    optional: Leaf | None
    values: list[Leaf]
    mapping: dict[str, Leaf]
    pair: tuple[int, str]
    hidden: int = field(default=0, compare=False, repr=False)


@dataclass(eq=False, slots=True)
class Identity:
    value: int


@dataclass(repr=False)
class Quiet:
    value: int


left = Record(Leaf("one", 2), None, [Leaf("two", 3)], {"three": Leaf("four", 5)}, (6, "seven"))
right = Record(Leaf("one", 2), None, [Leaf("two", 3)], {"three": Leaf("four", 5)}, (6, "seven"), hidden=9)
print(left == right)
print(left != right)
print(left)
print(str(left))
print(repr(left))
print(f"{left}")
print(f"{left!s}")
print(f"{left!r}")
print(f"{[Leaf('nested', 1)]!r}")
right.values[0].number = 10
print(left == right)
identity = Identity(1)
alias = identity
print(identity == alias)
print(identity == Identity(1))
print(Quiet(1) == Quiet(1))
print(str(Quiet(1))[0])
print(Leaf("same", 1) == OtherLeaf("same", 1))
print(Leaf("same", 1) != OtherLeaf("same", 1))
big_left = Leaf("big", int("1267650600228229401496703205376"))
big_right = Leaf("big", int("1267650600228229401496703205376"))
print(big_left == big_right)
big_right.number += 1
print(big_left == big_right)
big_right.number = 1
print(big_left == big_right)
text = "quote'\"\n\x00λ"
print(repr(text))
print(f"{text!r}")
binary = b"\x00\xff"
print(f"{binary!r}")
