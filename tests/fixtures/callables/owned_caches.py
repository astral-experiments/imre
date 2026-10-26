from functools import cache, lru_cache
from typing import Callable


def lookup(key: str, index: int) -> str:
    print(f"called {key} {index}")
    if index < 0:
        raise ValueError(key)
    return f"{key}:{index}"


def exercise() -> None:
    left = lru_cache(2)(lookup)
    right = lru_cache(2)(lookup)
    alias = left
    for index in (1, 2, 1, 3, 2, -1, -1):
        try:
            print(alias("x", index))
        except ValueError as error:
            print(str(error))
    print(right("x", 2))
    print(left("x", 2))
    lh, lm, lc, ln = left.cache_info()
    rh, rm, rc, rn = right.cache_info()
    print(f"{lh} {lm} {lc} {ln}; {rh} {rm} {rc} {rn}")
    left.cache_clear()
    print(alias.cache_info()[3])
    print(right.cache_info()[3])
    raw: Callable[[str, int], str] = alias.__wrapped__
    print(raw("x", 2))
    print(alias.__wrapped__("direct", 3))
    print(alias.cache_info()[1])
    uncached = lru_cache(-1)(lookup)
    print(uncached("u", 1))
    print(uncached("u", 1))
    uh, um, uc, un = uncached.cache_info()
    print(f"{uh} {um} {uc} {un}")
    unlimited = cache(lookup)
    print(unlimited("v", 8))
    print(unlimited("v", 8))
    h, m, c, n = unlimited.cache_info()
    print(f"{h} {m} {c} {n}")
    usual = lru_cache(lookup)
    print(usual("w", 1))
    print(usual("w", 1))
    print(usual.cache_info()[2])

    def local(n: int) -> int:
        print(f"local {n}")
        return n * 3

    for instance in range(2):
        inner = cache(local)
        outer = lru_cache(maxsize=1, typed=True)(inner)
        print(outer(instance))
        print(outer(instance))
        print(inner.cache_info()[1])
        print(outer.cache_info()[0])


exercise()
