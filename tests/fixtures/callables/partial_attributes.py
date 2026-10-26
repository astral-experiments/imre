from functools import Placeholder, partial
from typing import Callable


def combine(a: int, b: int, c: int) -> int:
    return 100 * a + 10 * b + c


def add_to(items: list[int], value: int) -> int:
    items.append(value)
    return len(items)


class Step:
    amount: int

    def __init__(self, amount: int) -> None:
        self.amount = amount

    def add(self, a: int, b: int) -> int:
        return self.amount + a + b


def use(fn: Callable[[int], int], value: int) -> int:
    return fn(value)


def choose(flag: bool) -> int:
    current = partial(combine, 1, 2)
    if flag:
        current = partial(combine, Placeholder, 4, 5)
    return current(7)


def main() -> None:
    first = partial(combine, Placeholder, 2)
    print(first.args[0] is Placeholder)
    print(first.args[1])
    print(first.args is first.args)
    print(first.func is combine)
    print(partial(combine).func is combine)
    print(first.func(1, 2, 3))
    print(first(4, 5))
    second = partial(first, 7)
    print(second.args == (7, 2))
    print(second.func is first.func)
    print(use(second, 9))
    empty = partial(second)
    print(empty is second)
    print(empty.func is second.func)
    print(empty.args is second.args)
    print(empty.args == (7, 2))
    print(empty(3))
    blank = ()
    print(partial(combine).args is blank)
    print(partial(combine, 1, 2, 3).args == (1, 2, 3))
    supplied = (3, 4)
    starred = partial(combine, *supplied)
    print(starred.args is supplied)
    print(starred.func is first.func)
    print(starred(5))
    items = [1]
    append = partial(add_to, items)
    args = append.args
    print(args[0] is items)
    print(args is append.args)
    print(append(9))
    print(len(args[0]))
    print(args[0][1])
    counter = Step(5)
    bound = partial(counter.add, 6)
    print(bound.func is bound.func)
    print(bound.func(2, 3))
    print(bound(4))
    print(choose(False))
    print(choose(True))


main()
