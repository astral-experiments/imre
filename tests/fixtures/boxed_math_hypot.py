import math
from math import hypot


class Coordinate:
    def __float__(self):
        return -12.0


def current():
    assert hypot is math.hypot
    assert hypot() == 0.0
    assert math.copysign(1.0, hypot(-0.0)) == 1.0
    assert hypot(Coordinate(), 3, 4) == 13.0
    scale = 2.0**900
    assert hypot(3 * scale, 4 * scale) == 5 * scale
    assert hypot(3 * 5e-324, 4 * 5e-324) == 5 * 5e-324
    assert hypot(572330659.1077356, 1180450261.5893514) == 1311878501.7832494
    assert hypot(math.nan, -math.inf) == math.inf
    assert math.isnan(hypot(math.nan, 1.0))
    try:
        hypot(math.inf, "bad")
    except TypeError:
        pass
    else:
        raise AssertionError("hypot skipped conversion after infinity")
    print("hypot ok")


if __name__ == "__main__":
    current()
