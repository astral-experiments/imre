from math import atan, copysign, pi


class Index:
    def __index__(self):
        return -1


class Real(Index):
    def __float__(self):
        return -0.0


def current():
    assert atan(Index()) == -pi / 4
    assert copysign(1.0, atan(Real())) == -1.0


if __name__ == "__main__":
    current()
