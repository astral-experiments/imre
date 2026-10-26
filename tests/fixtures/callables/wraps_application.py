from functools import wraps
from typing import Callable


def source(value: int) -> int:
    return value + 10


def outer(value: int) -> int:
    return value * 2


def choose(label: str, func: Callable[[int], int]) -> Callable[[int], int]:
    print(label)
    return func


def empty(label: str) -> tuple[()]:
    print(label)
    return ()


def local(base: int) -> int:
    def original(value: int) -> int:
        return value + 3

    @wraps(updated=empty("local updated"), wrapped=original, assigned=empty("local assigned"))
    def decorated(value: int) -> int:
        return value - 1

    print(decorated.__wrapped__ is original)
    print(decorated(base))
    return decorated.__wrapped__(base)


def exercise() -> None:
    result = wraps(updated=empty("updated"), assigned=empty("assigned"),
                   wrapped=choose("wrapped", source))(wrapper=choose("wrapper", outer))
    print(result is outer)
    print(result.__wrapped__ is source)
    print(result(5))
    print(result.__wrapped__(5))
    print(local(8))
    print(local(3))


exercise()
