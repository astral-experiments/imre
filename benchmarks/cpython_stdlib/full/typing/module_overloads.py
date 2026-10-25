"""Registration happens at definition time in an otherwise declarative entry."""
from typing import overload, get_overloads, clear_overloads


@overload
def handle(x: int) -> int: ...
@overload
def handle() -> int: ...
def handle(x: int = 8) -> int: return x


@overload
def missing(x: int) -> int: ...


def side_effect(items: list[int], value: int) -> int:
    items.append(value)
    return value


def main() -> None:
    recorded = get_overloads(handle)
    print(len(recorded))
    print(recorded[0] is recorded[1])
    print(recorded[0].__qualname__)
    print(recorded[1].__qualname__)
    print(handle() == 8)
    log: list[int] = []
    try:
        missing(side_effect(log, 2), keyword=side_effect(log, 3))
    except NotImplementedError as error:
        print(str(error))
    print(repr(log))
    clear_overloads()
    print(len(recorded))
    print(len(get_overloads(handle)))


main()
