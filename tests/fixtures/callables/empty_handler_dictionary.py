from typing import Callable


def double(x: int) -> int:
    return x * 2


def cold() -> None:
    unsolved: dict[str, Callable[[int], int]] = {}
    print(unsolved["x"](1))


print(double(21))
