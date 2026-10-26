from functools import lru_cache, partial, singledispatch
from typing import Callable


class Counter:
    value: int

    def __init__(self, start: int) -> None:
        self.value = start

    def add(self, step: int) -> int:
        self.value += step
        return self.value


def method(start: int) -> Callable[[int], int]:
    original = Counter(start).add
    first = partial(original)
    second = partial(*(original,))
    print(first is original)
    print(first is second)
    print(first(2))
    print(second(3))
    nested = partial(first)
    print(nested is first)
    return nested


def doubled(n: int) -> int:
    if n < 0:
        raise ValueError("negative")
    return n * 2


def wrap(target: Callable[[int], int]) -> Callable[[int], int]:
    return partial(target)


@lru_cache(maxsize=2)
def cached(n: int) -> int:
    print("miss")
    return n + 1


def dispatch() -> Callable[[int], int]:
    def plus_one(n: int) -> int:
        return n + 1

    first = singledispatch(doubled)
    second = singledispatch(doubled)
    saved = partial(first)
    separate = wrap(second)
    print(saved is first)
    print(saved(4))
    first.register(int, plus_one)
    print(saved(4))
    print(separate(4))
    return partial(saved)


def exercise() -> None:
    add = method(10)
    print(add(4))
    print(add(1))
    original = doubled
    copied = wrap(original)
    print(copied is original)
    print(copied(7))
    try:
        copied(-1)
    except ValueError as exc:
        print(str(exc))
    cache = cached
    via_partial = partial(cache)
    print(via_partial is cache)
    print(via_partial(3))
    print(cache(3))
    print(via_partial(4))
    print(cache.cache_info()[0])
    retained = dispatch()
    print(retained(7))


exercise()
