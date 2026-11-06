import copy
from itertools import repeat, tee
from sys import maxsize
from weakref import ref


def reenter_at_block_boundary():
    class Source:
        def __init__(self):
            self.index = 0
            self.other = None

        def __iter__(self):
            return self

        def __next__(self):
            index = self.index
            self.index += 1
            if index == 57:
                return next(self.other)
            return index

    source = Source()
    forward, backward = tee(source)
    source.other = backward
    for index in range(57):
        assert next(forward) == next(backward) == index
    try:
        next(forward)
    except RuntimeError as error:
        assert "tee" in str(error)
    else:
        raise AssertionError("recursive pull succeeded")
    assert next(forward) == next(backward) == 58
    assert source.index == 59


def release_lagging_block():
    class Token:
        pass

    def source():
        yield Token()
        yield from repeat(None, 57)

    forward, backward = tee(source())
    weak = ref(next(forward))
    assert not any(forward)
    assert weak() is not None
    assert next(backward) is weak()
    assert not any(backward)
    assert weak() is None


def reenter_when_releasing_block(prefetch):
    class Token:
        pass

    def source():
        yield Token()
        yield from range(1, 116)

    cursor, leader = tee(source())
    seen = []
    weak = ref(next(cursor), lambda _: seen.append(next(cursor)))
    if prefetch:
        for _ in range(115):
            next(leader)
    del leader

    for expected in range(1, 57):
        assert next(cursor) == expected
    first = 114 if prefetch else 57
    assert next(cursor) == first
    assert seen == [first] and weak() is None
    assert next(cursor) == first + 1


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

    reenter_at_block_boundary()
    release_lagging_block()
    reenter_when_releasing_block(False)
    reenter_when_releasing_block(True)
