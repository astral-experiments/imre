import math
from math import prod


def mutate(values: list[int], events: list[str]) -> int:
    events.append("start")
    values.append(-3)
    return -2


def source(values: list[int], events: list[str]) -> list[int]:
    events.append("source")
    return values


huge = 1 << 100
print(prod([]))
print(prod((), start=-huge))
print(math.prod([2, 2, 3, 3, 3, 17]))
print(prod([huge, -huge, 7], start=huge))
print(prod(range(1, 24)))
print(prod((4, -7, 3), start=-5))
print(prod(value * 2 for value in [2, 3, 4]))
print(prod(value > 0 for value in [2, -3, 4]))
print(prod([True, False], start=5))

events: list[str] = []
values = [5, 7]
print(prod(source(values, events), start=mutate(values, events)))
print(events)
print(values)

# Neither a zero factor nor a zero start may hide a later exception.
for start in [0, 1]:
    try:
        print(prod(map(int, ["0", "bad"]), start=start))
    except ValueError as error:
        print(error)

events = []
try:
    print(prod(range(1, 5, len(events)), start=mutate(values, events)))
except ValueError as error:
    print(error)
print(events)
