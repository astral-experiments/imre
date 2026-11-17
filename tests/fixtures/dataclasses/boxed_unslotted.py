from dataclasses import dataclass, field
@dataclass
class Item:
    value: int

def hold(value: object) -> object:
    return value
hold(Item(2))
print(hold(Item(2)))
