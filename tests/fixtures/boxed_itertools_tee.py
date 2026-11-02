import copy
import importlib
import sys
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

    def reject_foreign(owner, receiver):
        for method in (owner.__iter__, owner.__next__, owner.__copy__):
            try:
                method(receiver)
            except TypeError:
                pass
            else:
                raise AssertionError("tee method accepted a foreign receiver")

    class Other:
        def __init__(self, *args):
            assert not args, "tee tried calling the unrelated receiver's constructor"

        @property
        def _block(self):
            raise AssertionError("tee tried reading the unrelated receiver's state")

    old_fork, = tee([1], 1)
    old = type(old_fork)
    reject_foreign(old, Other())
    saved = sys.modules.pop("itertools")
    try:
        new_fork, = importlib.import_module("itertools").tee([2], 1)
        new = type(new_fork)
        assert old is not new
        reject_foreign(old, new_fork)
        reject_foreign(new, old_fork)
        assert list(old_fork) == [1] and list(new_fork) == [2]
    finally:
        sys.modules["itertools"] = saved

    assert tee(None, 0) == ()
    try:
        tee(None, maxsize)
    except MemoryError:
        pass
    else:
        raise AssertionError("oversized fork tuple")
