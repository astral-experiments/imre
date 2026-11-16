from dataclasses import dataclass, field
@dataclass
class Item:
    values: list[int] = field(default_factory=list())
