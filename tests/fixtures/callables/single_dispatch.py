import re
from functools import singledispatch


def base(value: str | bytes) -> str:
    return "base"


def text(value: str | bytes) -> str:
    return "text"


def replacement(value: str | bytes) -> str:
    return "replacement"


def documented(value: str | bytes) -> str:
    "  original\n\t  π\tcontinued\n\t    indented\n\n\t  final\n"
    return "original"


class TextBase:
    @staticmethod
    def static(value: str) -> str:
        "shared static documentation"
        return value


class TextChild(TextBase):
    pass


def wrapper_text(flag: bool) -> None:
    known = singledispatch(documented)
    no_doc = singledispatch(base)
    known.register(str, replacement)
    chosen = known if flag else no_doc
    print(chosen.__name__)
    doc = chosen.__doc__
    if doc is None:
        print("no docstring")
    else:
        print(repr(doc))
    print(known.__wrapped__ is documented)
    print(known("changed handler"))

    @singledispatch
    def nested(item):
        "  local\n    two\n\n      four\n"
        return "fallback"

    print(nested.__name__)
    local_doc = nested.__doc__
    if local_doc is not None:
        print(repr(local_doc))
    inherited = singledispatch(TextChild.static)
    print(inherited.__name__)
    inherited_doc = inherited.__doc__
    assert inherited_doc is not None
    print(inherited_doc)


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


def registered_format(text_or_bytes: str | bytes, optional: str | None) -> None:
    @singledispatch
    def select(item):
        return "base"

    @select.register(int)
    def show_number(item):
        return "int %s" % (item,)

    def show_ascii(item):
        return "%% %a" % (item,)

    print(select(""))
    print(select((re.compile("ignored"),)))
    print(select(12))
    print(select(True))
    print(select.dispatch(int) is show_number)
    print(select.dispatch(object) is select.dispatch(str))
    print(select.dispatch(int)("direct"))
    print(show_number(text_or_bytes))
    print(show_number(optional))
    print(show_number((1, "nested")))
    print(show_number(numbers(False)))
    print(show_number(int))
    print(show_ascii("π"))
    try:
        print(show_number(numbers(True)))
    except ValueError as exc:
        print(str(exc))


exercise(True)
exercise(False)
local_registry()
decorated()
decorated()
heterogeneous()
registered_format("literal", None)
registered_format(b"bytes", "present")
wrapper_text(True)
wrapper_text(False)
