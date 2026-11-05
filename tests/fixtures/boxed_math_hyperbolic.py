import math
from math import asinh, atanh, cosh, sinh, tanh


class Index:
    def __index__(self):
        return 1


class Real(Index):
    def __float__(self):
        return -0.0


def current():
    for func in (sinh, tanh, asinh, atanh):
        assert math.copysign(1.0, func(Real())) == -1.0
    assert cosh(Index()) == cosh(1)
    assert sinh(math.inf) == math.inf and tanh(-math.inf) == -1.0
    assert math.isnan(atanh(math.nan))

    edge = math.nextafter(1.0, 0.0)
    expected = 18.714973875118524
    assert abs(atanh(edge) - expected) <= math.ulp(expected)
    assert abs(atanh(-edge) + expected) <= math.ulp(expected)

    for func, arg, error in ((sinh, -1000, OverflowError), (cosh, 1000, OverflowError),
                             (atanh, 1.0, ValueError), (atanh, -math.inf, ValueError)):
        try:
            func(arg)
        except error:
            pass
        else:
            raise AssertionError((func, arg))

    math.sinh = tanh
    try:
        assert math.sinh(1.0) == tanh(1.0) and sinh(1.0) != tanh(1.0)
        del math.sinh
        assert not hasattr(math, "sinh")
    finally:
        math.sinh = sinh


if __name__ == "__main__":
    current()
