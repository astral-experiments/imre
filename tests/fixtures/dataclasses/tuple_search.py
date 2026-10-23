from dataclasses import dataclass


@dataclass
class Old:
    value: int


@dataclass
class New:
    value: int


def later(values: list[New]) -> list[New]:
    print("capture")
    return values


def first(old: list[Old], new: list[New]) -> int:
    for group in (old, later(new)):
        if group:
            return group[0].value
    return len(group)


print(first([Old(1)], [New(2)]))
print(first([], [New(3)]))
print(first([], []))
