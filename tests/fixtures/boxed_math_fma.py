import math
from math import fma


class FloatSubclass(float):
    def __float__(self):
        raise AssertionError("expected the stored double")


class Floatable:
    def __float__(self):
        return 2.5


class Indexed:
    def __index__(self):
        return 3


def current():
    assert fma is math.fma
    assert fma(FloatSubclass(2.0), Floatable(), Indexed()) == 8.0
    assert fma(1.0 + 2.0**-27, 1.0 - 2.0**-27, -1.0) == -(2.0**-54)
    assert math.copysign(1.0, fma(-0.0, 1.0, -0.0)) == -1.0
    inf = math.inf
    nan = math.nan
    assert math.isnan(fma(0.0, inf, nan))
    assert math.isnan(fma(nan, inf, -inf))
    assert fma(1.0, inf, 0.0) == inf
    assert fma(-inf, 2.0, -inf) == -inf
    for args, exception in [
        ((0.0, inf, 1.0), ValueError),
        ((inf, 2, -inf), ValueError),
        ((1e308, 2.0, 0.0), OverflowError),
        ((1, 2), TypeError),
        ((1, 2, "3"), TypeError),
    ]:
        try:
            fma(*args)
        except exception:
            pass
        else:
            raise AssertionError(args)
    try:
        fma(1.0, 2.0, z=3.0)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a keyword argument")


if __name__ == "__main__":
    current()
