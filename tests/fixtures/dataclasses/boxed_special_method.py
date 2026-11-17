from dataclasses import dataclass, field
@dataclass(slots=True)
class Item:
    value: int
    def __bool__(self) -> bool:
        return False

def hold(value: object) -> object:
    return value
hold(Item(2))
print(bool(hold(Item(2))))
