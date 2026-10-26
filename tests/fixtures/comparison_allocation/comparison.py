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


left = int(sys.argv[2])
right = int(sys.argv[2])
if sys.argv[1] == "direct":
    print(direct(Frame((left, 4), 5), Frame((right, 4), 5)))
elif sys.argv[1] == "boxed":
    print(boxed(((((((left, 4), 5), 6), 7), 8), 9), ((((((right, 4), 5), 6), 7), 8), 9)))
else:
    print(boxed(1, 2))
