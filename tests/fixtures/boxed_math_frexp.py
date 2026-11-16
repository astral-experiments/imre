import math
from math import frexp


class Floatable:
    def __float__(self):
        return -6.0


class Indexed:
    def __index__(self):
        return 3


def current():
    assert frexp is math.frexp
    result = frexp(Floatable())
    assert type(result) is tuple and result == (-0.75, 3)
    assert type(result[0]) is float and type(result[1]) is int
    assert frexp(Indexed()) == (0.75, 2)
    assert frexp(5e-324) == (0.5, -1073)
    for zero in (0.0, -0.0):
        mantissa, exponent = frexp(zero)
        assert mantissa == 0.0 and exponent == 0
        assert math.copysign(1.0, mantissa) == math.copysign(1.0, zero)
    assert frexp(-math.inf) == (-math.inf, 0)
    mantissa, exponent = frexp(math.nan)
    assert math.isnan(mantissa) and exponent == 0
    try:
        frexp(1 << 1024)
    except OverflowError:
        pass
    else:
        raise AssertionError("integer overflow was lost")
    print(result)


if __name__ == "__main__":
    current()
