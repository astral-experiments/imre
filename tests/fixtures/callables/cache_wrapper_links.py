from functools import lru_cache, update_wrapper
from typing import Callable


def double(n: int) -> int:
    return n * 2


def successor(n: int) -> int:
    return n + 1


@lru_cache(4, typed=True)
def declared(n: int) -> int:
    return n * 3


def read(owner: Callable[[int], int]) -> Callable[[int], int]:
    return owner.__wrapped__


def relink(owner: Callable[[int], int]) -> None:
    update_wrapper(owner, successor, (), ())


def exercise() -> None:
    cached = lru_cache(4)(double)
    alias = cached
    original = read(alias)
    print(original(5))
    relink(alias)
    print(read(cached)(5))
    print(cached(5))
    print(original is double, cached.__wrapped__ is successor, alias is cached)
    print(cached(5))
    info = cached.cache_info()
    print(info[0], info[1], info[2], info[3])
    print(repr(cached.cache_parameters()))
    update_wrapper(declared, successor, (), ())
    print(declared.__wrapped__ is successor, declared.__wrapped__(5), declared(5))


exercise()
