import math
from decimal import Decimal


def current():
    close = math.isclose
    from math import isclose

    assert close is isclose
    assert close(a=9, b=10, rel_tol=0.1) is True
    assert close(1, 1 + 1e-8) is False
    inf = float("inf")
    nan = float("nan")
    assert close(inf, inf) and not close(-inf, inf, abs_tol=inf)
    assert not close(nan, nan, abs_tol=inf)
    assert close(1, 2, rel_tol=nan, abs_tol=1)

    assert close(Decimal("1.125"), 1, abs_tol=Decimal("0.125"))
    assert Decimal.__float__(Decimal("1.125")) == 1.125
    assert math.copysign(1, float(Decimal("-0"))) == -1
    class WrongReceiver:
        _text = "1.25"

    try:
        Decimal.__float__(WrongReceiver())
    except TypeError:
        pass
    else:
        raise AssertionError("accepted non-Decimal receiver")
    qnan = float(Decimal("-NaN123"))
    assert math.isnan(qnan) and math.copysign(1, qnan) == -1
    assert not close(Decimal("NaN42"), 1)
    try:
        close(Decimal("sNaN123"), 0)
    except ValueError as error:
        assert str(error) == "cannot convert signaling NaN to float"
    else:
        raise AssertionError("accepted signaling NaN")

    seen = []

    class Operand:
        def __init__(self, value):
            self.value = value

        def __float__(self):
            seen.append(self.value)
            return self.value

    assert not close(b=Operand(2.0), a=Operand(1.0), abs_tol=Operand(0.0), rel_tol=Operand(0.1))
    assert seen == [1.0, 2.0, 0.1, 0.0]
    for call, error in (
        (lambda: close(Operand(7.0), 2, 0), TypeError),
        (lambda: close(1, b=2, a=3), TypeError),
        (lambda: close(1, 1, abs_tol=-1), ValueError),
        (lambda: close("1", 1), TypeError),
        (lambda: close(10**1000, 1), OverflowError),
    ):
        try:
            call()
        except error:
            pass
        else:
            raise AssertionError(error)
    assert seen == [1.0, 2.0, 0.1, 0.0]
    print("boxed math.isclose")


if __name__ == "__main__":
    current()
