from itertools import chain


def current():
    calls = []

    class Later:
        def __iter__(self):
            calls.append("opened")
            return iter([2])

    cursor = chain([], [1], Later())
    assert iter(cursor) is cursor and next(cursor) == 1 and not calls
    assert next(cursor) == 2 and calls == ["opened"]
    assert next(cursor, None) is None and next(cursor, None) is None

    cursor = chain([1], None, [2])
    assert next(cursor) == 1
    try:
        next(cursor)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a non-iterable")
    assert next(cursor, None) is None

    class PrivateState(chain):
        def __setattr__(self, name, value):
            raise AssertionError(name)

        def __getattribute__(self, name):
            if name in ("_sources", "_active"):
                raise AssertionError(name)
            return super().__getattribute__(name)

    assert list(PrivateState([1], [2])) == [1, 2]

    class Constructor(chain):
        def __new__(cls, *args):
            raise AssertionError("__new__")

        def __init__(self, *args):
            raise AssertionError("__init__")

        def __setattr__(self, name, value):
            raise AssertionError(name)

    cursor = Constructor.from_iterable([[3], [4]])
    assert type(cursor) is Constructor and list(cursor) == [3, 4]
