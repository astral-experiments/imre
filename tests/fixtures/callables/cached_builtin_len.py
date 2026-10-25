import builtins
import functools


def call_len(text: str) -> int:
    return len(text)


def rebind_while_evaluating_argument() -> list[str]:
    builtins.len = functools.lru_cache(1)(len)
    return ["🐍", "é"]


def rebind_while_evaluating_source() -> list[str]:
    builtins.len = functools.lru_cache(1)(len)
    return ["again", "again"]


def pipeline() -> None:
    words = ["éé", "x", "éé"]
    print(list(map(len, words)))
    print(list(map(len, [b"", b"cd"])))
    original = builtins.len
    wrapper = functools.lru_cache(2)(original)
    try:
        builtins.len = wrapper
        print(list(map(len, words)))
        print(wrapper.cache_info() == (1, 2, 2, 2))
        print(list(filter(len, ["x", "", "x"])))
        print(wrapper.cache_info() == (3, 3, 2, 2))
        try:
            print(list(map(len, [[1], [2]])))
        except TypeError as error:
            print(str(error))
        print(wrapper.cache_info() == (3, 3, 2, 2))
        print(list(map(len, rebind_while_evaluating_source())))
        print(wrapper.cache_info() == (4, 4, 2, 2))
    finally:
        builtins.len = original
    print(builtins.len is original)


def exercise() -> None:
    original = builtins.len
    print(original is len)
    wrapper = functools.lru_cache(2)(original)
    try:
        builtins.len = wrapper
        print(builtins.len is wrapper)
        print(call_len("🐍é"))
        print(call_len("🐍é"))
        print(call_len("a"))
        print(call_len(""))
        print(wrapper.cache_info() == (1, 3, 2, 2))
        builtins.len = original
        print(len(rebind_while_evaluating_argument()))
        try:
            print(len(["a", "b"]))
        except TypeError as error:
            print(str(error))
        print(wrapper.cache_info() == (1, 3, 2, 2))
        print(call_len("abc"))
        print(original("abc"))
        print(wrapper.__wrapped__ is original)
        raise ValueError("restore")
    except ValueError as error:
        print(str(error))
    finally:
        builtins.len = original
    print(builtins.len is original)
    print(call_len("abc"))
    print(wrapper("🐍é"))
    print(wrapper.cache_info() == (1, 4, 2, 2))
    wrapper.cache_clear()
    print(wrapper(((True, 2),)))
    print(wrapper(((1, 2),)))
    print(wrapper.cache_info() == (1, 1, 2, 1))
    print(wrapper(("ab", b"c")))
    print(wrapper(("a", b"bc")))
    print(wrapper.cache_info() == (1, 3, 2, 2))
    try:
        print(wrapper(([1],)))
    except TypeError as error:
        print(str(error))
    print(wrapper.cache_info() == (1, 3, 2, 2))
    uncached = functools.lru_cache(0)(original)
    print(uncached([1, 2, 3]))
    print(uncached.cache_info() == (0, 1, 0, 0))


class Marker:
    def __init__(self, value: int) -> None:
        self.value = value


def mutable_elements() -> None:
    values = [Marker(7)]
    entries = {"x": values[0]}
    original = builtins.len
    print(original(values))
    print(original(entries))
    cache = functools.lru_cache(2)(original)
    try:
        print(cache((values,)))
    except TypeError as error:
        print(str(error))
    try:
        print(cache(entries))
    except TypeError as error:
        print(str(error))
    print(cache.cache_info() == (0, 0, 2, 0))


exercise()
pipeline()
mutable_elements()
