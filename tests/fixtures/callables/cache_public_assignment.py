from functools import lru_cache


def double(n: int) -> int:
    return n * 2


def successor(n: int) -> int:
    return n + 1


@lru_cache(4, typed=True)
def triple(n: int) -> int:
    return n * 3


def run() -> None:
    cache = lru_cache(4)(double)
    alias = cache
    original = alias.__wrapped__
    cache.__wrapped__ = successor
    print(original is double, alias.__wrapped__ is successor)
    print(alias.__wrapped__(5), alias(5), cache(5))
    print(cache.cache_info().hits)
    triple.__wrapped__ = successor
    print(triple.__wrapped__ is successor, triple.__wrapped__(5), triple(5))


run()
