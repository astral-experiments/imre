from dataclasses import dataclass, field
@dataclass(slots=True, eq=False)
class Item:
    value: int

def hold(value: object) -> object:
    return value
hold(Item(2))
left = Item(2)
right = Item(2)
print(hold(left) == hold(left), hold(left) == hold(right))
