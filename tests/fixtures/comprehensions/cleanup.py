"""Unpublished aggregates release their owners on filter/element/iterator errors."""

from typing import TextIO


class Payload:
    number: int | None
    values: list[int]
    text: str

    def __init__(self, number: int | None, values: list[int], text: str) -> None:
        self.number = number
        self.values = values
        self.text = text


def payload(index: int) -> Payload:
    if index == 3:
        raise ValueError("element failure")
    return Payload(
        1000000000000000000000000000000 + index, [index], "owned " + str(index)
    )


def filter_value(index: int) -> bool:
    if index == 3:
        raise ValueError("filter failure")
    return True


def dict_key(index: int) -> str:
    return "owned key " + str(index)


def nested_source(index: int) -> list[int]:
    if index == 2:
        raise ValueError("inner iterable failure")
    return [index, index + 1]


def open_part(prefix: str, index: int) -> TextIO:
    if index == 2:
        raise ValueError("stop opening")
    stream = open(prefix + str(index) + ".txt", "w", encoding="utf-8")  # noqa: SIM115 — transferred owner
    stream.write(prefix + str(index))
    return stream


def files() -> None:
    try:
        _opened = [open_part("list", index) for index in range(4)]
    except ValueError:
        print("partial list files released")
    try:
        _mapped = {dict_key(index): open_part("dict", index) for index in range(4)}
    except ValueError:
        print("partial dict files released")
    with open("list0.txt", "r", encoding="utf-8") as stream:
        print(stream.read())
    with open("list1.txt", "r", encoding="utf-8") as stream:
        print(stream.read())
    with open("dict0.txt", "r", encoding="utf-8") as stream:
        print(stream.read())
    with open("dict1.txt", "r", encoding="utf-8") as stream:
        print(stream.read())


def failing_return() -> list[Payload]:
    try:
        return [payload(index) for index in range(3)]
    finally:
        raise ValueError("discard successful comprehension return")


def reraise_at_one(index: int) -> int:
    if index == 1:
        raise  # noqa: PLE0704 — the active handler is dynamically scoped
    return index


def active_exception() -> None:
    try:
        raise ValueError("outer handler remains active")
    except ValueError:
        try:
            _values = [reraise_at_one(index) for index in range(3)]
        except ValueError as error:
            print(str(error))
    try:
        _values = [reraise_at_one(index) for index in range(3)]
    except RuntimeError:
        print("helper sees no active exception after handler")


def main() -> None:
    # Repeated successful recovery exposes leaked partial builders and captures
    # under LeakSanitizer without relying on user finalizers or refcount APIs.
    for turn in range(24):
        try:
            _result = [payload(index) for index in range(5)]
        except ValueError:
            pass
        try:
            _result = [payload(index) for index in range(5) if filter_value(index)]
        except ValueError:
            pass
        try:
            _mapped = {dict_key(index): payload(index) for index in range(5)}
        except ValueError:
            pass
        try:
            _nested = [
                [payload(inner) for inner in nested_source(outer)] for outer in range(4)
            ]
        except ValueError:
            pass
        try:
            _unique = {dict_key(index) for index in range(5) if filter_value(index)}
        except ValueError:
            pass
        try:
            failing_return()
        except ValueError:
            pass
    print("recovered partial aggregates")
    files()
    active_exception()


main()
