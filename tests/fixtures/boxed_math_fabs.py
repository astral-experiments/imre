from math import copysign, fabs


class Indexed:
    def __index__(self):
        return -5

    def __abs__(self):
        raise AssertionError("fabs must convert to float")


class Real(Indexed):
    def __float__(self):
        return -2.5


def current():
    assert fabs(Indexed()) == 5.0
    assert fabs(Real()) == 2.5
    assert copysign(1.0, fabs(-0.0)) == 1.0


if __name__ == "__main__":
    current()
