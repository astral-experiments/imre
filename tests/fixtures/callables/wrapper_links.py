from functools import update_wrapper
from typing import Callable


def outer(value: int) -> int:
    return value - 1


def inner(value: int) -> int:
    return value + 10


def other(value: int) -> int:
    return value * 4


def empty(label: str) -> tuple[()]:
    print(label)
    return ()


def make() -> Callable[[int], int]:
    def wrapper(value: int) -> int:
        return value

    def original(value: int) -> int:
        return value * 2

    return update_wrapper(wrapper, original, (), ())


def exercise() -> None:
    alias = outer
    try:
        alias.__wrapped__
    except AttributeError as error:
        print(str(error))
    result = update_wrapper(updated=empty("updated"), wrapped=inner,
                            assigned=empty("assigned"), wrapper=outer)
    print(result is alias)
    print(alias.__wrapped__ is inner)
    print(f"{alias(5)} {alias.__wrapped__(5)}")
    saved = alias.__wrapped__
    update_wrapper(alias, other, (), ())
    print(f"{saved is inner} {alias.__wrapped__ is other}")
    print(f"{saved(5)} {alias.__wrapped__(5)}")
    left = make()
    right = make()
    print(left is not right)
    print(left.__wrapped__ is not right.__wrapped__)
    print(left.__wrapped__(6))
    update_wrapper(left, right, (), ())
    update_wrapper(right, left, (), ())
    print(left.__wrapped__.__wrapped__ is left)
    update_wrapper(right, right, (), ())
    print(right.__wrapped__ is right)


exercise()
