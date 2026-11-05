import gc
import math
import weakref
from math import dist


class Point(tuple):
    def __iter__(self):
        raise AssertionError("tuple coordinates should come from storage")


class Cursor:
    def __init__(self):
        self.items = iter((3.0,))
    def __iter__(self):
        return self
    def __next__(self):
        return next(self.items)

def cursor_distance(retained):
    refs, owners = [], []
    class Left:
        def __iter__(self):
            cursor = Cursor()
            refs.append(weakref.ref(cursor))
            if retained:
                owners.append(cursor)
            return cursor
    def right():
        gc.collect()
        yield 4.0 if refs[0]() is not None else 0.0
    return math.dist(Left(), right())


def current():
    assert cursor_distance(True) == 1.0
    assert cursor_distance(False) == 3.0
    assert dist is math.dist
    assert dist(Point((1, 2, 3)), iter((4, 2, -1))) == 5.0
    assert math.copysign(1.0, dist((-0.0,), (0.0,))) == 1.0
    assert dist((math.inf, math.nan), (0.0, 1.0)) == math.inf
    assert math.isnan(dist((math.inf,), (math.inf,)))
    # Both iterables are consumed before coordinates are converted or rejected.
    left = [3.0, 4.0]

    def right():
        left.clear()
        yield 0
        yield 0

    assert dist(left, right()) == 5.0
    try:
        dist([object()], [0, 1])
    except ValueError:
        pass
    else:
        raise AssertionError("unequal dimensions")
    print("dist ok")


if __name__ == "__main__":
    current()
