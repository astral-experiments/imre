from dataclasses import dataclass, field
@dataclass(slots=True)
class Item:
    value: list[int]

def hold(value: object) -> object:
    return value
hold(Item([2]))
print(hold(Item([2])))
