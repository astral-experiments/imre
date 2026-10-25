from functools import cache, lru_cache


@cache
def unbounded(n: int) -> str:
    print(f"miss {n}")
    if n < 0:
        raise ValueError("negative")
    if n == 5:
        unbounded.cache_clear()
    return str(n)


@lru_cache(2)
def bounded(x: str, y: int) -> str:
    print(f"miss {x} {y}")
    return x


@lru_cache(maxsize=-42)
def disabled() -> int:
    print("uncached")
    return 17


@lru_cache
def default(n: int) -> int:
    return n * n


def main() -> None:
    print(unbounded.cache_info()[0])
    for n in [1, 1, -1, -1, 2, 2, 5, 5]:
        try:
            print(unbounded(n))
        except ValueError as error:
            print(str(error))
        hits, misses, size, count = unbounded.cache_info()
        print(f"{hits} {misses} {size} {count}")
    for i in range(90):
        for key in [0, 1, 0, 2, 0, 1, i]:
            print(bounded("key", key))
    h, m, cap, count = bounded.cache_info()
    print(f"{h} {m} {cap} {count}")
    bounded.cache_clear()
    print(bounded.cache_info()[3])
    print(disabled())
    print(disabled())
    h2, m2, cap2, count2 = disabled.cache_info()
    print(f"{h2} {m2} {cap2} {count2}")
    print(default(9))
    print(default(9))
    print(default.cache_info()[0])


main()
