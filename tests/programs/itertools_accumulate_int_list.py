from itertools import accumulate as scan
from operator import add


def start(values: list[int]) -> int:
    values.append(5)
    return -3


def prefixes(values: list[int]) -> None:
    print(list(scan(values)), list(scan(values, add)))
    print(list(scan(values, initial=None)), list(scan(values, initial=0)))
    print(list(scan(initial=start(values), func=add, iterable=values)))


prefixes([])
prefixes([1, -3, 4])
wide = 1 << 110
prefixes([wide, -wide, wide - 1, 1])


def difference(a: int, b: int) -> int:
    if b == 4:
        raise StopIteration
    return a - b


print(list(scan(range(7), difference)))
print(sum(scan([2, 3, 4, 5], difference, initial=10)))
print(list(scan([2.5, -3.0, 1.25], initial=0.5)))


def saved(values: list[int]) -> list[int]:
    iterator = scan(values, initial=0)
    alias = iterator
    print(next(alias), next(iterator))
    values.append(-5)
    print(list(alias))
    values.append(99)
    return list(iterator)


print(saved([1 << 100, -(1 << 100), 7]))
