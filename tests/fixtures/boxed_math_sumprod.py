import math
from math import sumprod


def current():
    assert sumprod is math.sumprod
    assert sumprod(iter([2, 3]), [5, 7]) == 31
    delta = 2.0**-27
    assert sumprod([1.0 + delta, -1.0], [1.0 - delta, 1.0]) == -(delta**2)

    class Factor:
        def __rmul__(self, left):
            assert left == 3
            factors.append(2)
            return 12

    factors = [Factor()]
    assert sumprod([3, 4], factors) == 20

    right = iter([1, 2, 3])
    try:
        sumprod([5], right)
    except ValueError as error:
        assert str(error) == "Inputs are not the same length"
    else:
        raise AssertionError("accepted unequal lengths")
    assert next(right) == 3


if __name__ == "__main__":
    current()
