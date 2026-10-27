"""Rounding function objects and the original operands they dispatch on."""

import math
from math import ceil, floor, trunc


class Hooks:
    def __trunc__(self):
        return self.marker

    def __floor__(self):
        return self.marker

    def __ceil__(self):
        return self.marker


class Number(Hooks):
    pass


class Integral(int):
    def __ceil__(self):
        return self


class Real(float):
    def __floor__(self):
        return self


class Before(Hooks, int):
    pass


class After(int, Hooks):
    pass


class FloatOnly:
    def __float__(self):
        return -1.25


class IndexOnly:
    def __index__(self):
        return 5


def current():
    big = (1 << 200) + 1
    obj = Number()
    obj.marker = []
    obj.__floor__ = lambda: None
    for op, original in ((trunc, math.trunc), (floor, math.floor), (ceil, math.ceil)):
        assert op is original and not hasattr(op, "__get__")
        assert op(obj) is obj.marker and op(big) is big
        print(op(3), op(True), op(-2.75), op(2.25), op(-0.0), op(1e100))
        for bad in (float("nan"), float("-inf"), "2.5", None):
            try:
                op(bad)
            except (TypeError, OverflowError, ValueError) as error:
                print(type(error).__name__, str(error))
        for call in (lambda: op(), lambda: op(1, 2), lambda: op(x=2)):
            try:
                call()
            except TypeError as error:
                print(str(error))
    for value in (FloatOnly(), IndexOnly()):
        print(floor(value), ceil(value))
        try:
            trunc(value)
        except TypeError as error:
            print(str(error))
    before = int.__new__(Before, big)
    before.marker = obj.marker
    after = int.__new__(After, big)
    for op in (trunc, floor, ceil):
        assert op(before) is obj.marker
        assert type(op(after)) is int and op(after) == big
    integer = int.__new__(Integral, 5)
    real = Real()
    assert ceil(integer) is integer and floor(real) is real


if __name__ == "__main__":
    current()
