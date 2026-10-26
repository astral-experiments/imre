from functools import _CacheInfo, cache, lru_cache


@lru_cache(2)
def square(n: int) -> int:
    return n * n


@cache
def identity(n: int) -> int:
    return n


def mark(n: int) -> int:
    print(n)
    return n


def plain(n: int) -> int:
    return n


def main() -> None:
    before = square.cache_info()
    square(3)
    square(4)
    square(3)
    after = square.cache_info()
    square.cache_clear()
    print(repr(before))
    print(repr(after))
    print(repr(square.cache_info()))
    print(f"{after.hits} {after.misses} {after.maxsize} {after.currsize}")
    print(after[0])
    print(repr(after[::-1]))
    print(after == (1, 2, 2, 2))
    print(after is before)
    print(before is square.cache_info())
    alias = after
    print(after is alias)
    print(repr(after._fields))
    print(repr(_CacheInfo.__match_args__))
    print(_CacheInfo.__name__)
    print(after == _CacheInfo(hits=1, misses=2, maxsize=2, currsize=2))
    print(repr(_CacheInfo(currsize=mark(4), hits=mark(1), maxsize=mark(3), misses=mark(2))))
    identity(6)
    print(identity.cache_info() == _CacheInfo(0, 1, None, 1))
    print(identity.cache_info().maxsize is None)
    print(repr(_CacheInfo("first", False, None, "last")))
    owned = lru_cache(1)(plain)
    owned(12)
    owned(12)
    print(repr(owned.cache_info()))
    nested = lru_cache(1)(identity)
    nested(12)
    nested(12)
    print(repr(nested.cache_info()))


main()
