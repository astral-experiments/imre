from typing import Callable


def for_finally(values: list[int]) -> int:
    result: int | None = 1
    for value in values:
        result = value
        try:
            break
        finally:
            result = None
    else:
        result = 1
    return result + 1


def while_finally(value: int | None) -> int:
    while value is not None:
        try:
            break
        finally:
            value = None
    else:
        value = 1
    return value + 1


def while_break(value: int | None) -> int:
    while value is None:
        break
    else:
        print(value + 1)
    return value + 1


def while_exhausted(value: int | None) -> int:
    while value:
        value = None
    else:
        return value + 1


def probe(operation: Callable[[int | None], int], value: int | None) -> None:
    try:
        print(operation(value))
    except TypeError as error:
        print(str(error))


print(for_finally([]))
try:
    print(for_finally([3]))
except TypeError as error:
    print(str(error))
probe(while_finally, None)
probe(while_finally, 3)
probe(while_break, 3)
probe(while_break, None)
probe(while_exhausted, 0)
probe(while_exhausted, 3)
probe(while_exhausted, None)
