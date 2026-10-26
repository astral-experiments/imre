"""Supplemental differential checks for source position, mutation and exceptions."""
from http import HTTPStatus
from itertools import count, cycle, islice
from operator import length_hint


def consume(selector: int) -> None:
    src = count(10)
    if selector:
        sub = islice(src, 5, 2)
    else:
        sub = islice(src, 1, 4, 999)
    print(repr(list(sub)))
    print(next(src))
    print(next(sub, -1))


def grow(mapping: dict[str, int]) -> int:
    mapping["late"] = 9
    return 2


def nullable_bounds(start: int | None, stop: HTTPStatus | None, step: int | None) -> None:
    source = iter(range(210))
    section = islice(source, start, stop, step)
    print((next(section), len(list(section)), next(source, -1)))


def boolean_bounds(start: bool | None, stop: bool | None, step: bool | None) -> None:
    source = iter(range(4))
    try:
        print((list(islice(source, start, stop, step)), next(source, -1)))
    except ValueError as error:
        print((str(error), next(source, -1)))


def main() -> None:
    consume(0)
    consume(1)
    nullable_bounds(None, HTTPStatus.OK, 3)
    nullable_bounds(7, None, None)
    boolean_bounds(True, None, None)
    boolean_bounds(None, True, True)
    boolean_bounds(False, None, False)
    source = [1, 2]
    c = cycle(source)
    print(next(c))
    source[1] = 8
    source.append(9)
    print(repr(list(islice(c, 5))))
    source[0] = 7
    print(repr(list(islice(c, 3))))
    r = iter(range(10, 0, -2))
    print(repr(list(islice(r, 2))))
    print(next(r))
    print(length_hint(r))
    try:
        length_hint(iter(range(1 << 90)))
    except OverflowError:
        print("range hint overflow")
    mapping = {"early": 1}
    items = islice(mapping, grow(mapping))
    print(repr(list(items)))
    broken = islice(mapping, None)
    print(next(broken))
    mapping["changed"] = 4
    try:
        next(broken)
    except RuntimeError:
        print("source mutated")
    print(next(broken, "done"))
    print(repr(list(islice("é猫🙂a", 1, None, 2))))
    print(repr(list(islice(b"123", 2))))
    print(repr(list(islice((30, 20, 10), 1, None))))
    big = count(1 << 110, -(1 << 90))
    print(repr(big))
    print(next(big))
    print(repr(big))
    fractional = count(step=1.0, start=2.5)
    print(repr(fractional))
    print(next(fractional))
    print(repr(fractional))
    huge = count(2.5, 1 << 2000)
    try:
        next(huge)
    except OverflowError:
        print("step conversion on pull")
    try:
        islice("a", "bad")
    except ValueError as error:
        print(str(error))
    try:
        islice("a", 2, -1)
    except ValueError as error:
        print(str(error))


main()
