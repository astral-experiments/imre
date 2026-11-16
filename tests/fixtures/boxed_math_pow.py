import math
from math import pow as real_power


class StoredFloat(float):
    def __float__(self):
        raise AssertionError("expected the stored double")


class Exponent:
    def __index__(self):
        return 3


class Base:
    def __float__(self):
        return -2.0


def current():
    assert real_power is math.pow
    assert type(real_power(2, 3)) is float
    assert real_power(StoredFloat(-3), 2) == 9.0
    assert real_power(Base(), Exponent()) == -8.0
    assert real_power(9, 0.5) == 3.0
    assert real_power(math.nan, 0) == real_power(1, math.nan) == 1.0
    assert math.isnan(real_power(math.nan, 3))
    assert math.isnan(real_power(2, math.nan))
    assert real_power(-1, math.inf) == 1.0
    assert real_power(-math.inf, 3) == -math.inf
    assert real_power(-math.inf, 0.5) == math.inf
    assert math.copysign(1, real_power(-math.inf, -3)) == -1.0
    assert real_power(-0.0, -math.inf) == math.inf
    assert math.copysign(1, real_power(-0.0, 3)) == -1.0
    assert math.copysign(1, real_power(-1e-200, 3)) == -1.0

    for base, exponent in [(-1, 0.5), (0, -2), (-0.0, -3)]:
        try:
            real_power(base, exponent)
        except ValueError as error:
            assert str(error) == "math domain error"
        else:
            raise AssertionError("expected a domain error")

    try:
        real_power(1e300, 2)
    except OverflowError as error:
        assert str(error) == "math range error"
    else:
        raise AssertionError("expected overflow")

    for args, kwargs, error_type in [
        ((1,), {}, TypeError),
        ((1, 2, 3), {}, TypeError),
        ((), {"x": 2, "y": 3}, TypeError),
        ((1j, 0), {}, TypeError),
        ((1, 10**400), {}, OverflowError),
    ]:
        try:
            real_power(*args, **kwargs)
        except error_type:
            pass
        else:
            raise AssertionError("expected an argument error")
    print("math.pow checks passed")


if __name__ == "__main__":
    current()
