from dataclasses import dataclass, field


@dataclass(frozen=True, slots=True, kw_only=True)
class Snapshot:
    name: str
    values: list[int] = field(default_factory=list)

    @property
    def count(self) -> int:
        return len(self.values)


first = Snapshot(name="first")
second = Snapshot(name="second")
first.values.append(4)
print(first.count)
print(second.count)
print(first)
