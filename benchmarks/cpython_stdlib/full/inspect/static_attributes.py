from dataclasses import dataclass
from annotationlib import get_annotations
from inspect import _sentinel as sentinel, getattr_static
import inspect


@dataclass
class Row:
    value: int


@dataclass
class Labels:
    items: list[str]


def receiver(row: Row) -> Row:
    print("receiver")
    return row


def change(row: Row) -> int:
    print("default")
    row.value += 1
    return 19


def omitted(default=inspect._sentinel) -> bool:
    return default is sentinel


def annotated(value: str) -> bool:
    return bool(value)


def run() -> None:
    row = Row(17)
    first = object()
    second = object()
    print(first is second)
    print(first is first)
    print(sentinel is inspect._sentinel)
    print(first is not sentinel)
    print(omitted())
    print(omitted(first))
    print(omitted(sentinel))
    print(getattr_static(row, "value", change(row)))
    print(getattr_static(default=change(row), obj=receiver(row), attr="value"))
    print(getattr_static(row, "missing", first) is first)
    print(getattr_static(object(), "missing", 42))
    print(getattr_static(False, "missing", None) is None)
    try:
        getattr_static(row, "missing", sentinel)
    except AttributeError as error:
        print(str(error))
        print(repr(error))
    try:
        getattr_static(first, "missing")
    except AttributeError as error:
        print(str(error))
        print(repr(error))
    lower = getattr_static("value", "lower", change(row))
    print(lower is str.lower)
    print(lower is not str.strip)
    print(lower == str.lower)
    print(lower != None)
    print(lower.__name__)
    print(lower.__qualname__)
    print(lower.__objclass__ is str)
    print(get_annotations(annotated)["value"] is lower.__objclass__)
    print(inspect.ismethoddescriptor(lower))
    print(inspect.ismethoddescriptor(object=first))
    print(inspect.ismethoddescriptor(str))
    print(repr(lower))
    print(lower("İßΜΈΣΟΣ"))
    print(str.lower("ΣΟΣ"))
    strip = getattr_static("value", "strip")
    print(strip(" x "))
    print(strip("xyaxy", "xy"))
    print(strip(" x ", None))
    print(getattr_static("value", "lstrip")("xax", "x"))
    print(getattr_static("value", "rstrip")("xax", "x"))
    print(repr(first).startswith("<object object at 0x"))
    saved = getattr_static(Labels(["temporary"]), "items")
    saved.append("retained")
    print(saved)
    holder = Labels(["before"])
    previous = getattr_static(holder, "items")
    holder.items = ["after"]
    previous.append("independent")
    print(previous)
    print(holder.items)


run()
