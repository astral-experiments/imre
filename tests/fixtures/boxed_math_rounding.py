"""Rounding function objects and the original operands they dispatch on."""

import math
from math import ceil, copysign, floor, isnan, modf, trunc


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


class Xéééééééééééééééééééééééééééé:
    pass


class FloatOverride(float):
    def __float__(self):
        raise AssertionError("modf consulted a float subclass override")


def current():
    assert float.__trunc__(Real(-1.75)) == -1
    assert type(int.__trunc__(True)) is int
    assert bool.__trunc__ is int.__trunc__
    assert int.__trunc__.__objclass__ is int
    assert float.__trunc__.__get__(2.5, float)() == 2

    pi = math.pi
    assert pi == 3.141592653589793 and vars(math)["pi"] is pi
    math.pi = None
    assert math.pi is None
    del math.pi
    assert not hasattr(math, "pi")
    math.pi = pi
    assert math.pi is pi

    assert isnan is math.isnan
    assert isnan(FloatOverride("nan")) is True
    for value in (float("inf"), -0.0, FloatOnly(), IndexOnly()):
        assert isnan(value) is False

    assert copysign is math.copysign
    assert copysign(FloatOnly(), IndexOnly()) == 1.25
    assert copysign(IndexOnly(), FloatOverride(-0.0)) == -5.0
    assert type(copysign(True, False)) is float
    assert str(copysign(0, -1)) == "-0.0"
    assert copysign(float("inf"), -0.0) == -float("inf")
    negative_nan = copysign(float("nan"), -0.0)
    assert isnan(negative_nan) and copysign(1, negative_nan) == -1.0
    for call, error_type, message in (
        (lambda: copysign(1), TypeError, "copysign expected 2 arguments, got 1"),
        (lambda: copysign(1, 2, 3), TypeError, "copysign expected 2 arguments, got 3"),
        (lambda: copysign(1, y=-1), TypeError, "math.copysign() takes no keyword arguments"),
        (lambda: copysign(1, "2"), TypeError, "must be real number, not str"),
        (lambda: copysign(10**1000, "2"), OverflowError, "int too large to convert to float"),
    ):
        try:
            call()
        except error_type as error:
            assert str(error) == message
        else:
            raise AssertionError(message)

    assert math.modf(FloatOnly()) == (-0.25, -1.0)
    assert modf(IndexOnly()) == (0.0, 5.0)
    assert modf(FloatOverride(-1.5)) == (-0.5, -1.0)
    assert modf(True) == (0.0, 1.0)
    try:
        modf(Xéééééééééééééééééééééééééééé())
    except TypeError as error:
        assert str(error) == "must be real number, not " + "X" + "é" * 24
    else:
        raise AssertionError("modf accepted a nonnumeric class")
    for value in (-0.0, -1.0, -float("inf")):
        fraction, integral = modf(value)
        assert str(fraction) == "-0.0" and str(integral) == str(value)
    fraction, integral = modf(float("nan"))
    assert fraction != fraction and integral != integral
    for bad, error in (("1.5", TypeError), (10**1000, OverflowError)):
        try:
            modf(bad)
        except error:
            pass
        else:
            raise AssertionError("modf accepted an invalid operand")

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
