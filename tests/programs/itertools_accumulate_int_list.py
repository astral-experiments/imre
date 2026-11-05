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
