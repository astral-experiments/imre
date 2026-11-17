from bisect import bisect_left, bisect_right
from collections import namedtuple

Row = namedtuple("Row", "left right")


def search(values: tuple[int, ...], needle: int) -> None:
    print(bisect_left(values, needle), bisect_right(values, needle))
    print(
        bisect_left(values, needle, 0, -1),
        bisect_right(values, needle, lo=0, hi=None, key=None),
    )


search((), 0)
search(tuple([-2**100, 1, 2, 2, 2**100]), 2)
search(tuple([-2**100, 1, 2, 2, 2**100]), 2**100)
search(Row(1, 3), 2)
