from collections import defaultdict as factory_dict
from typing import Callable
from unittest import TestCase


def mark(label: str, value: int) -> int:
    print(label)
    return value


def later_error() -> int:
    print("later argument")
    raise ValueError("before factory validation")


def typed(factory: int) -> None:
    try:
        result: factory_dict[str, list[int]] = factory_dict(factory)
    except TypeError as error:
        print(str(error))


def shadowed(list: int) -> None:
    try:
        factory_dict(list)
    except TypeError as error:
        print(str(error))


def increment(value: int) -> int:
    return value + 1


def local_constructor(defaultdict: Callable[[int], int]) -> int:
    return defaultdict(7)


class Value:
    label: str

    def __init__(self, label: str) -> None:
        self.label = label


class Case(TestCase):
    def concrete_factory(self) -> None:
        self.assertRaises(TypeError, factory_dict, {"k": 1})
        self.assertRaises(TypeError, factory_dict, 1)


try:
    factory_dict({})
except TypeError as error:
    print(str(error))
try:
    factory_dict([])
except TypeError as error:
    print(str(error))
try:
    factory_dict(Value("value"))
except TypeError as error:
    print(str(error))
try:
    factory_dict(mark("factory", 1), mark("positional", 2), named=mark("keyword", 3))
except TypeError as error:
    print(str(error))
try:
    factory_dict(mark("factory before raising argument", 1), later_error())
except ValueError as error:
    print(str(error))
try:
    factory_dict("bad", {}, [], named={})
except TypeError as error:
    print(str(error))
typed(0)
shadowed(2)
print(local_constructor(increment))
case = Case()
case.concrete_factory()
print("both callback invocations raised TypeError")
alive: factory_dict[str, list[int]] = factory_dict(list)
alive["key"].append(42)
print(alive["key"])
