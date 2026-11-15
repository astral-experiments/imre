from dataclasses import dataclass, field
@dataclass
class Item:
    value: int
    @property
    def double(self) -> int:
        return self.double

print(Item(3).value)
