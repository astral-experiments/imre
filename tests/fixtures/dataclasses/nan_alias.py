from dataclasses import dataclass


@dataclass
class Number:
    value: float


shared = float("nan")
first = Number(shared)
second = Number(shared)
print(first == first)
print(first == second)
