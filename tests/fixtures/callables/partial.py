from functools import Placeholder, partial
from typing import Callable


def combine(a: int, b: int, c: int) -> int:
    return 100 * a + 10 * b + c


def noted(x: int) -> int:
    print(f"bind {x}")
    return x


def make(n: int) -> Callable[[int, int], int]:
    return partial(combine, noted(n))


def choose(reverse: bool) -> Callable[[int, int], int]:
    if reverse:
        return partial(combine, Placeholder, 2)
    return make(3)


class Counter:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value

    def increase(self, x: int, y: int) -> int:
        self.value += x + y
        return self.value


def add_to(items: list[int], value: int) -> int:
    items.append(value)
    return len(items)


def main() -> None:
    f = make(2)
    print(f(3, 4))
    print(f(4, 3))
    print(partial(combine, 1, Placeholder, noted(3))(7))
    print(partial(combine, 9, 8, 7)())
    for flag in [True, False]:
        print(partial(choose(flag), 6)(9))
    counter = Counter(5)
    inc = partial(counter.increase, 2)
    print(inc(3))
    print(inc(4))
    items = [1]
    append = partial(add_to, items)
    print(append(8))
    items.append(9)
    print(append(10))
    words = list(map(str, range(3)))
    print(partial(str.join, "↔")(words))
    print(partial("é".join)(words))
    for item in items:
        print(item)
    try:
        partial(combine, noted(5), Placeholder)
    except TypeError as exc:
        print(str(exc))
    try:
        partial(2, noted(6))()
    except TypeError as exc:
        print(str(exc))
    try:
        partial()
    except TypeError as exc:
        print(str(exc))
    try:
        partial(combine, noted(7), c=Placeholder)
    except TypeError as exc:
        print(str(exc))


main()
