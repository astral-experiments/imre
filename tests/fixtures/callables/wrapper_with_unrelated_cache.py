from functools import cache, update_wrapper
from typing import Callable


def plain(n: int) -> int:
    return n - 1


def added(n: int) -> int:
    return n + 10


def cache_body(n: int) -> int:
    return n * 3


def invoke(fn: Callable[[int], int]) -> int:
    return fn(4)


def exercise() -> None:
    alias = plain
    unrelated = cache(cache_body)
    result = update_wrapper(plain, added, (), ())
    print(result is alias, alias.__wrapped__ is added, alias.__wrapped__(5))
    print(invoke(unrelated), unrelated.cache_info()[1])


exercise()
