"""Compare actual callback binding errors and the position of their exception boundary."""

import unittest


class Worker:
    def operation(self, value: str = "default", /) -> None:
        print("entered " + value)
        raise ValueError(value)


def many(a: int, b: int, c: int, /, optional: int = 0, *, required: int) -> None:
    raise ValueError("entered many")


def pair(first: int, second: int = 0) -> None:
    raise ValueError("entered pair")


def mark(text: str) -> str:
    print("evaluated " + text)
    return text


def unrelated_argument() -> str:
    raise TypeError("from argument")


def exercise() -> None:
    case = unittest.TestCase()
    raises = case.assertRaises
    worker = Worker()
    try:
        raises(LookupError, Worker.operation)
    except TypeError as e:
        print(str(e))
    try:
        raises(LookupError, worker.operation, mark("one"), mark("two"))
    except TypeError as e:
        print(str(e))
    raises(TypeError, worker.operation, "unused", {})
    raises(ValueError, Worker.operation, worker, "unbound")
    raises(ValueError, worker.operation)
    try:
        raises(TypeError, worker.operation, unrelated_argument(), mark("must not evaluate"))
    except TypeError as e:
        print(str(e))
    try:
        raises(LookupError, many)
    except TypeError as e:
        print(str(e))
    try:
        raises(LookupError, many, 1, 2, 3)
    except TypeError as e:
        print(str(e))
    try:
        raises(LookupError, many, 1, 2, 3, 4, 5, required=6)
    except TypeError as e:
        print(str(e))
    raises(ValueError, many, 1, 2, 3, required=4)
    try:
        raises(LookupError, pair, 1, first=2)
    except TypeError as e:
        print(str(e))
    try:
        raises("invalid", pair, {})
    except TypeError as e:
        print(str(e))


exercise()
