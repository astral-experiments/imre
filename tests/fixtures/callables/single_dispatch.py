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


def numbers(fail: bool) -> list[int]:
    print("evaluate numbers")
    if fail:
        raise ValueError("from argument")
    return [1, 2, 3]


def heterogeneous() -> None:
    @singledispatch
    def classify(item):
        return "other"

    def integer(item):
        return "integer"

    print(classify.register(int, integer) is integer)
    print(classify("s"))
    print(classify(9))
    print(classify(True))
    print(classify(numbers(False)))
    print(integer(numbers(False)))
    print(classify.dispatch(int)(b"direct"))
    try:
        print(classify(numbers(True)))
    except ValueError as exc:
        print(str(exc))


exercise(True)
exercise(False)
local_registry()
decorated()
decorated()
heterogeneous()
