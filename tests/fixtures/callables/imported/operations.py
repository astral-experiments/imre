from collections.abc import Callable


def increment(value: int) -> int:
    return value + 1


def decrement(value: int) -> int:
    return value - 1


def apply(fn: Callable[[int], int], value: int) -> int:
    return fn(value)


def choose(flag: bool) -> Callable[[int], int]:
    return increment if flag else decrement
