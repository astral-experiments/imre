import copy
from itertools import tee
from sys import maxsize


def current():
    seen = []

    def source():
        for item in range(130):
            seen.append(item)
            yield item

    first, slow = tee(source())
    assert seen == [] and iter(first) is first
    for expected in range(75):
        assert next(first) == expected
    clone = copy.copy(first)
    nested, = tee(first, 1)
    assert clone is not first and nested is not first
    assert next(slow) == 0 and next(clone) == next(nested) == 75
    assert seen == list(range(76))
    assert list(slow) == list(range(1, 130))
    assert list(first) == list(range(75, 130))
    assert list(clone) == list(nested) == list(range(76, 130))
    assert seen == list(range(130))

    assert tee(None, 0) == ()
    try:
        tee(None, maxsize)
    except MemoryError:
        pass
    else:
        raise AssertionError("oversized fork tuple")
