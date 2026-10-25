from functools import singledispatch


def base(value: str | bytes) -> str:
    return "base"


def text(value: str | bytes) -> str:
    return "text"


def replacement(value: str | bytes) -> str:
    return "replacement"


def exercise(flag: bool) -> None:
    first = singledispatch(base)
    other = singledispatch(base)
    print(first("initial"))
    print(first.register(str, text) is text)
    print(first("item"))
    print(first(b"item"))
    print(other("item"))
    print(first.dispatch(str) is text)
    print(first.dispatch(bytes) is base)
    old = first.dispatch(str)
    first.register(str, replacement)
    print(old("item"))
    print(first("item"))
    print(first.register(int, text) is text)
    print(first.dispatch(bool) is text)
    cls = bool if flag else bytes
    print(first.dispatch(cls) is text)
    first.register(bool, replacement)
    print(first.dispatch(bool) is replacement)
    first.register(object, replacement)
    print(first.dispatch(bytes) is replacement)
    print(first(b"changed base"))
    print(first.__wrapped__ is base)
    print(first.__wrapped__(b"original"))
    print(other.dispatch(bool) is base)


exercise(True)
exercise(False)
