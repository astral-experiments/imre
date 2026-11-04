import math
from math import ldexp


def current():
    assert ldexp is math.ldexp
    assert math.ldexp(1.5, 3) == 12.0
    assert ldexp(6993274598585239, -1126) == 1e-323
    assert math.copysign(1, ldexp(-1, -(1 << 100))) == -1
    try:
        ldexp(1, 1 << 100)
    except OverflowError as exc:
        assert str(exc) == "math range error"
    else:
        raise AssertionError("expected overflow")
    try:
        ldexp(0, 1.5)
    except TypeError as exc:
        assert str(exc) == "Expected an int as second argument to ldexp."
    else:
        raise AssertionError("expected an integer exponent")
    print("boxed math.ldexp")


if __name__ == "__main__":
    current()
