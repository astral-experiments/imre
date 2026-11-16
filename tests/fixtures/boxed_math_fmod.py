from math import copysign, fmod, inf, isnan


def current():
    calls = []

    class Dividend:
        def __float__(self):
            calls.append("float")
            return -6.0

    class Divisor:
        def __index__(self):
            calls.append("index")
            return 3

    assert copysign(1.0, fmod(Dividend(), Divisor())) == -1.0
    assert calls == ["float", "index"]
    assert fmod(-2.0, inf) == -2.0
    assert isnan(fmod(float("nan"), 0))
    try:
        fmod(inf, 3)
    except ValueError as exc:
        assert str(exc) == "math domain error"
    else:
        raise AssertionError("infinite dividend")
