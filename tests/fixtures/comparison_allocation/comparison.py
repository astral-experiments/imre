from collections import OrderedDict
from dataclasses import dataclass
import sys


@dataclass(slots=True)
class Frame:
    pair: tuple[int, int]
    tail: int


def direct(left: Frame, right: Frame) -> str:
    try:
        return str(left == right)
    except MemoryError as error:
        return repr(error) + "|" + str(error)


def boxed(left: object, right: object) -> str:
    try:
        return str(left == right)
    except MemoryError as error:
        return repr(error) + "|" + str(error)


def mapping(ordered: bool, n: int) -> None:
    left: OrderedDict[tuple[tuple[int, int], int], int] = OrderedDict()
    right: dict[tuple[tuple[int, int], int], int] = {}
    if ordered:
        right = left.copy()
    left[((n, n + 1), n)] = 7
    right[((n, n + 1), n)] = 7
    try:
        print(left == right)
    except MemoryError as error:
        print(repr(error) + "|" + str(error))
    print(len(left))
    print(len(right))
    print(left[((n, n + 1), n)])
    print(right[((n, n + 1), n)])
    print(left == right)


left = int(sys.argv[2])
right = int(sys.argv[2])
if sys.argv[1] == "direct":
    print(direct(Frame((left, 4), 5), Frame((right, 4), 5)))
elif sys.argv[1] == "boxed":
    print(boxed(((((((left, 4), 5), 6), 7), 8), 9), ((((((right, 4), 5), 6), 7), 8), 9)))
elif sys.argv[1] == "ordered" or sys.argv[1] == "plain":
    mapping(sys.argv[1] == "ordered", left)
else:
    print(boxed(1, 2))
