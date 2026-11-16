import math

saved = math.gamma
marker = object()
math.extra = marker
del math.gamma
try:
    from math import *
finally:
    math.gamma = saved
    del math.extra


def current():
    assert extra is marker and "gamma" not in globals()
    assert cbrt(-8) == -2.0 and erf(0.0) == 0.0
    assert math.gamma(13.0) == 479001600.0 and lgamma(1.0) == 0.0

    class Real:
        def __float__(self):
            return -0.0

    assert math.copysign(1.0, expm1(Real())) == -1.0
    for op in (exp2, expm1):
        try:
            op(1024.0)
        except OverflowError as error:
            assert str(error) == "math range error"
        else:
            raise AssertionError(op)
