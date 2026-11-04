import math
from math import acosh


class Two:
    def __float__(self):
        return 2.0


def current():
    assert acosh is math.acosh
    assert acosh(1) == 0.0
    assert math.isclose(acosh(1.0000000000000002), 2.1073424255447017e-8, rel_tol=1e-15)
    assert math.isclose(acosh(2**1023), 709.782712893384, rel_tol=1e-15)
    assert acosh(Two()) == acosh(2)
    assert math.isnan(acosh(float("nan")))
    assert acosh(float("inf")) == float("inf")
    for x in (0.9999999999999999, -0.0, float("-inf")):
        try:
            acosh(x)
        except ValueError as error:
            assert str(error) == f"expected argument value not less than 1, got {x}"
        else:
            raise AssertionError(x)
    for call, error in (
        (lambda: acosh(2**1024), OverflowError),
        (lambda: acosh(-(2**1024)), OverflowError),
        (lambda: acosh("2"), TypeError),
        (lambda: acosh(x=2), TypeError),
    ):
        try:
            call()
        except error:
            pass
        else:
            raise AssertionError(error)
    print("boxed math.acosh")


if __name__ == "__main__":
    current()
