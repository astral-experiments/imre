from dataclasses import dataclass, field
@dataclass
class Item:
    value: int
    @classmethod
    def make(cls, value: int) -> "Item":
        return cls.make(value)

print(Item(3).value)
