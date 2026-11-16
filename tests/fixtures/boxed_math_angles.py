from math import copysign, degrees, pi, radians


class Indexed:
    def __index__(self):
        return 180


class Real(Indexed):
    def __float__(self):
        return -0.0


def current():
    assert radians(Indexed()) == pi
    assert copysign(1.0, degrees(Real())) == -1.0
    assert degrees(1e308) == float("inf")


if __name__ == "__main__":
    current()
