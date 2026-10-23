from dataclasses import dataclass, field


@dataclass
class Batch:
    name: str
    items: list[str] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.items.append(self.name)
        print("|".join(self.items))
        if not self.name:
            raise ValueError("empty batch")


@dataclass
class Manual:
    value: int

    def __init__(self, value: int):
        self.value = value

    def __post_init__(self) -> None:
        print("manual hook")


first = Batch("first")
second = Batch("second")
print("|".join(first.items))
print("|".join(second.items))
try:
    Batch("")
except ValueError as exc:
    print(str(exc))
manual = Manual(42)
print(manual.value)
manual.__post_init__()
