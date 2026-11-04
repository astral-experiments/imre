from collections import namedtuple
from time import gmtime


class Override(tuple):
    def __iter__(self):
        return iter(("override",))


def current():
    slot = tuple.__iter__
    assert slot.__objclass__ is tuple
    assert type(slot).__name__ == "wrapper_descriptor"
    assert list(("one", "two").__iter__()) == ["one", "two"]
    value = Override((1, 2))
    assert list(iter(value)) == ["override"]
    cursor = slot(value)
    assert type(cursor).__name__ == "tuple_iterator"
    assert cursor.__iter__() is cursor
    assert type(cursor).__next__(cursor) == 1
    assert list(cursor) == [2]
    assert next(cursor, None) is None
    Point = namedtuple("Point", "x y")
    assert list(slot(Point(3, 4))) == [3, 4]
    stamp = gmtime(0)
    assert type(iter(stamp)) is type(cursor)
    assert list(slot(stamp)) == [1970, 1, 1, 0, 0, 0, 3, 1, 0]

    try:
        slot([])
    except TypeError:
        pass
    else:
        raise AssertionError("tuple iterator accepted a list")
