import builtins
import functools


def call_len(text: str) -> int:
    return len(text)


def replacement(text: str) -> int:
    return 42


def rebind_while_evaluating_argument() -> str:
    builtins.len = replacement
    return "🐍é"


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
        print(len(rebind_while_evaluating_argument()))
        print(wrapper.cache_info() == (1, 4, 2, 2))
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
    print(wrapper.cache_info() == (2, 4, 2, 2))


exercise()
