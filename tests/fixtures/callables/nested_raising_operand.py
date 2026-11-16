from typing import Callable


def target(first: int, second: int) -> int:
    print("entered target")
    return first + second


def choose() -> Callable[[int, int], int]:
    print("callee")
    return target


def mark() -> int:
    print("argument")
    raise KeyError("earlier")


def stop(items: set[int]) -> str:
    items.add(mark(), 2)
    raise ValueError("later")


try:
    print(choose()(stop({19}), True))
except KeyError:
    print("caught earlier")
