import math
from math import prod


def current():
    sentinel = object()
    assert prod is math.prod
    assert prod([]) == 1
    assert prod((), start=sentinel) is sentinel
    assert prod(iter((2, 3, 4)), start=0.5) == 12.0
    assert prod((2, 1.5, 4)) == 12.0
    assert prod([2**90, 2**70]) == 2**160

    assert prod((2**45, 2**45, 3)) == 3 * 2**90

    class Extend:
        def __rmul__(self, left):
            factors.append(3)
            return left + 1

    factors = [2, 3, Extend()]
    assert prod(factors) == 21

    class Factor:
        def __rmul__(self, left):
            assert left == 0
            raise ValueError("after zero")

    factors = iter([2, 0, Factor(), 9])
    try:
        prod(factors)
    except ValueError as error:
        assert str(error) == "after zero"
    else:
        raise AssertionError("skipped a factor")
    assert next(factors) == 9

    for args, kwargs in [(([], 1), {}), ((), {"iterable": []}), (([],), {"initial": 1})]:
        try:
            prod(*args, **kwargs)
        except TypeError:
            pass
        else:
            raise AssertionError((args, kwargs))


if __name__ == "__main__":
    current()
