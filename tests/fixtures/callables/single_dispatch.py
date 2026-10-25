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


def local_registry() -> None:
    def local_base(value: str) -> str:
        return "local base"

    def local_text(value: str) -> str:
        return "local text"

    registry = singledispatch(local_base)
    print(registry.__wrapped__ is local_base)
    print(registry.register(str, local_text) is local_text)
    print(registry.dispatch(str) is local_text)
    print(registry("item"))


def select_text() -> bool:
    print("select str")
    return True


def decorated() -> None:
    @singledispatch
    def choose(value: str | bytes) -> str:
        return "unregistered"

    print(choose("before"))

    @choose.register(str if select_text() else bytes)
    def handle_text(value: str | bytes) -> str:
        return "decorated text"

    print(choose.dispatch(str) is handle_text)
    print(choose("after"))
    print(choose(b"bytes"))
    saved = choose.dispatch(str)
    print(choose.register(str if select_text() else bytes)(replacement) is replacement)
    print(choose("replaced"))
    print(saved("saved"))
    print(choose.__wrapped__("original"))


exercise(True)
exercise(False)
local_registry()
decorated()
decorated()
