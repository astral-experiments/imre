from functools import cache, lru_cache
from typing import Callable


def identity(value: int) -> int:
    return value


@lru_cache(maxsize=2, typed=True)
def declared(value: int) -> int:
    return value


def choose() -> Callable[[int], int]:
    print("choose")
    return identity


def choose_cached(flag: bool) -> Callable[[int], int]:
    print(flag)
    if flag:
        return declared
    return identity


def report_erased(fn: Callable[[int], int]) -> None:
    outer = lru_cache(3)(fn)
    print(repr(outer.cache_parameters()))
    print(outer.cache_info()[2])
    print(outer(7))
    print(outer(7))


def call(fn: Callable[[int], int], value: int) -> int:
    return fn(value)


def exercise() -> None:
    first = declared.cache_parameters()
    print(repr(first))
    first["maxsize"] = 99
    first["typed"] = False
    print(repr(first))
    print(repr(declared.cache_parameters()))

    disabled = lru_cache(-5, typed=True)(identity)
    print(repr(disabled.cache_parameters()))
    unlimited = cache(identity)
    print(repr(unlimited.cache_parameters()))
    print(repr(lru_cache(identity).cache_parameters()))
    print(repr(lru_cache(3)(choose()).cache_parameters()))

    outer = lru_cache(maxsize=1)(unlimited)
    print(repr(outer.cache_parameters()))
    print(outer.cache_info()[2])
    inherited = lru_cache(4)(declared)
    print(repr(inherited.cache_parameters()))
    print(inherited.cache_info()[2])
    alias = declared
    print(repr(alias.cache_parameters()))
    print(repr(lru_cache(3)(alias).cache_parameters()))
    print(call(alias, 7))
    print(declared.cache_info()[3])
    alias.cache_clear()
    print(declared.cache_info()[3])

    report_erased(choose_cached(False))
    report_erased(choose_cached(True))
    report_erased(unlimited)
    print(unlimited.cache_info()[3])
    report_erased(lru_cache(9223372036854775807, typed=True)(identity))


exercise()
