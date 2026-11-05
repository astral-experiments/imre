import math
from math import log2


class Real:
    def __float__(self):
        return 0.125


class Index:
    def __index__(self):
        return 32


class Integer(int):
    def __float__(self):
        raise AssertionError("converted an int subclass through __float__")


def current():
    assert log2 is math.log2
    for exponent in (0, 1, 52, 53, 1023, 1024, 4096):
        assert log2(Integer(1 << exponent)) == exponent
    assert log2(5e-324) == -1074
    assert log2(3 << 4096) == 4096.0 + log2(3)
    assert log2(Real()) == -3.0 and log2(Index()) == 5.0
    assert log2(True) == 0.0
    assert math.isnan(log2(float("nan")))
    assert log2(float("inf")) == float("inf")
    for value in (0, False, -(1 << 4096), 0.0, -0.0, -1.0, float("-inf")):
        try:
            log2(value)
        except ValueError as error:
            message = "expected a positive input"
            if isinstance(value, float):
                message += f", got {value}"
            assert str(error) == message
        else:
            raise AssertionError(value)
    for call in (lambda: log2(), lambda: log2(1, 2), lambda: log2(x=2), lambda: log2("2")):
        try:
            call()
        except TypeError:
            pass
        else:
            raise AssertionError("accepted invalid arguments")
    print("boxed math.log2")


if __name__ == "__main__":
    current()
