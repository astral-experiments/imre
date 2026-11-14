from typing import Sequence
def count(xs: Sequence[int]) -> int:
    return len(xs)
print(count((1, 2)))


def read(values: Sequence[int], live: list[int]) -> int:
    total = 0
    for item in values:
        if item == 1:
            live.append(3)
        total += item
    return total + len(values) + values[-1]


live = [1, 2]
print(read(live, live), live)
print(read((1, 2), live), live)
