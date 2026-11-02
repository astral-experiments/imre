from collections import deque
import sys


def current():
    marker = object()
    original = deque([marker, 2, 3], 5)
    for result in (original * 2, 2 * original):
        assert result is not original and result.maxlen == 5
        assert list(result) == [2, 3, marker, 2, 3] and result[2] is marker
    old = original
    original *= 42
    assert original is old and list(original) == [2, 3, marker, 2, 3]
    original *= 0
    assert not original and original.maxlen == 5

    for values, limit, invalid in (([0], 1, True), ([0, 1], 2, False)):
        d = deque(values, limit)
        cursor = iter(d)
        d *= 10
        try:
            item = next(cursor)
        except RuntimeError:
            assert invalid
        else:
            assert not invalid and item == 0
    assert list(deque([marker], 3) * sys.maxsize) == [marker] * 3
    try:
        deque([1, 2], 2) * sys.maxsize
    except MemoryError:
        pass
    else:
        raise AssertionError("overflow before applying the bound")
    try:
        deque() * (sys.maxsize + 1)
    except OverflowError:
        pass
    else:
        raise AssertionError("accepted oversized multiplier")

    class Index:
        def __index__(self):
            d.append(3)
            return 2

    d = deque([1, 2])
    assert list(d * Index()) == [1, 2, 3] * 2

    events = []
    class Derived(deque):
        def __init__(self, values):
            events.append("init")
            super().__init__(values)

        def __iter__(self):
            events.append("iter")
            return iter([8, 9])

        def copy(self):
            raise AssertionError("invoked overridden copy")

    d = Derived([1, 2])
    events.clear()
    repeated = d * 2
    assert type(repeated) is Derived and list(deque.__iter__(repeated)) == [8, 9, 8, 9]
    assert events == ["init", "iter", "iter"]
    events.clear()
    d *= 2
    assert list(deque.__iter__(d)) == [1, 2, 8, 9] and events == ["iter"]

    class Right:
        def __rmul__(self, other):
            return marker

    assert deque() * Right() is marker
    assert deque.__imul__(d, 1) is d
