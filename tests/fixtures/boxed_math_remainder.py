import math
from math import remainder


def current():
    assert math.remainder is remainder
    assert remainder(5, 2) == 1.0 and remainder(7, 2) == -1.0
    assert remainder(-7, -2) == 1.0
    assert math.copysign(1.0, remainder(-360, 360)) == -1.0
    tiny = float.fromhex('0x1p-1074')
    assert remainder(3 * tiny, 2 * tiny) == -tiny
    assert remainder(-3, math.inf) == -3.0
    assert math.isnan(remainder(math.nan, 0.0))
    assert math.isnan(remainder(math.inf, math.nan))

    for x, y in [(1, -0.0), (math.inf, 1)]:
        try:
            remainder(x, y)
        except ValueError as exc:
            assert str(exc) == "math domain error"
        else:
            raise AssertionError((x, y))

    original = math.remainder
    try:
        math.remainder = math.copysign
        assert math.remainder(7.0, 2.0) == 7.0 and original(7.0, 2.0) == -1.0
        del math.remainder
        assert not hasattr(math, "remainder")
    finally:
        math.remainder = original
    print("math.remainder checks passed")


if __name__ == "__main__":
    current()
