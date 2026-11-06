from math import asin, copysign


class Real:
    def __float__(self):
        return -0.0

    def __index__(self):
        raise AssertionError("called __index__ after __float__")


def current():
    assert copysign(1.0, asin(Real())) == -1.0
    try:
        asin(2)
    except ValueError as error:
        assert str(error) == "expected a number in range from -1 up to 1, got 2.0"
    else:
        raise AssertionError("accepted an out-of-domain input")


if __name__ == "__main__":
    current()
