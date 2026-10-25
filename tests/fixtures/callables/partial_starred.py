from functools import Placeholder, partial
from typing import Callable


def number(a: int, b: int, c: int, d: int) -> int:
    return 1000 * a + 100 * b + 10 * c + d


def choose(events: list[int]) -> Callable[[int, int, int, int], int]:
    events.append(0)
    return number


def pair(events: list[int], n: int) -> tuple[int, int]:
    events.append(n)
    if n < 0:
        raise ValueError("spread failed")
    return (n, n + 1)


def keep(items: list[int], n: int) -> int:
    items.append(n)
    return len(items)


def exercise() -> None:
    events: list[int] = []
    make = partial(choose(events), *pair(events, 1), *(), *pair(events, 3))
    print(make())
    print(make())
    print(events)

    holes = (Placeholder, 5)
    fill = partial(number, *holes, *(Placeholder, 7))
    print(fill(4, 6))
    print(fill(8, 9))
    print(partial(*(number, 3), *(), 4)(5, 6))
    packed = partial(*(number, 3), *(), 4)
    print(packed(7, 8))
    all_open = partial(*(), number, *())
    print(all_open is number)
    print(all_open(9, 8, 7, 6))

    values = [2]
    retained = partial(keep, *(values,))
    values.append(3)
    print(retained(4))
    print(retained(5))
    print(values)

    try:
        partial(choose(events), *pair(events, -1), *pair(events, 8))
    except ValueError as error:
        print(str(error))
        print(events)
    try:
        partial(number, *holes, *(Placeholder,))
    except TypeError:
        print("trailing placeholder")
    try:
        partial(*(3, 4), *pair(events, 6))
    except TypeError:
        print("invalid target")
        print(events)
    try:
        partial(*())
    except TypeError:
        print("missing target")


exercise()
