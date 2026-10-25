from functools import reduce
from typing import Callable


def subtract(left: int, right: int) -> int:
    print(left * 10 + right)
    return left - right


def callback() -> Callable[[int, int], int]:
    print("callback")
    return subtract


def source() -> list[int]:
    print("source")
    return [1, 2, 3]


def seed() -> int:
    print("seed")
    return 9


def append(total: list[str], item: str) -> list[str]:
    total.append(item)
    return total


def main() -> None:
    print(reduce(callback(), source(), seed()))
    print(reduce(subtract, [8, 4, 2]))
    print(reduce(lambda x, y: x * y, range(2, 22), 1))
    print(reduce(lambda x, y: x + y, "🦀ab"))
    out: list[str] = []
    print(len(reduce(append, "abc", out)))
    print("".join(out))
    print(reduce(42, "x"))
    print(reduce(42, "", "seed"))
    try:
        print(reduce(subtract, range(0)))
    except TypeError as error:
        print(str(error))
    try:
        print(reduce(42, "xy"))
    except TypeError as error:
        print(str(error))
    try:
        print(reduce(lambda x, y: x // y, [0, 1], 1))
    except ZeroDivisionError:
        print("callback failed")


main()
