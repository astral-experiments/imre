from dataclasses import dataclass, field


@dataclass
class Counter:
    count: int = 0
    reads: int = 0
    items: list[int] = field(default_factory=list)

    @property
    def next_value(self) -> int:
        self.reads += 1
        self.count += 1
        return self.count

    @property
    def values(self) -> list[int]:
        return self.items

    @property
    def checked(self) -> int:
        if self.count < 0:
            raise ValueError("negative")
        return self.count


counter = Counter()
print(f"{counter.next_value} {counter.next_value} {counter.reads}")
counter.values.append(9)
print(sum(counter.items))
print(counter.checked)
counter.count = -1
try:
    print(counter.checked)
except ValueError as error:
    print(str(error))
print(counter.reads)
