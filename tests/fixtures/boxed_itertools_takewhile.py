from itertools import takewhile


def current():
    events = []

    class Source:
        def __init__(self, items):
            self.items = iter(items)

        def __iter__(self):
            events.append("iter")
            return self

        def __next__(self):
            item = next(self.items)
            events.append(item)
            if isinstance(item, BaseException):
                raise item
            return item

    class Truth:
        def __bool__(self):
            raise ArithmeticError("truth")

    def predicate(value):
        if value == 2:
            raise ValueError("predicate")
        return Truth() if value == 3 else value

    source = Source([1, OSError("source"), 2, 3, StopIteration("pause"), 4, 0, 5])
    try:
        takewhile.__new__(int, predicate, source, extra=True)
    except TypeError:
        pass
    else:
        raise AssertionError("constructed an unrelated type")
    assert not events
    cursor = takewhile(predicate, source)
    assert iter(cursor) is cursor and events == ["iter"]
    assert next(cursor) == 1
    for kind in (OSError, ValueError, ArithmeticError, StopIteration):
        try:
            next(cursor)
        except kind:
            pass
        else:
            raise AssertionError("iterator did not propagate the error")
    assert next(cursor) == 4 and next(cursor, None) is None
    assert next(cursor, None) is None and next(source) == 5

    class Cursor(takewhile):
        @property
        def _state(self):
            raise AssertionError("read a subclass attribute")

        def __init__(self, *args, label):
            self.label = label

    cursor = Cursor(bool, [1, 2, 0, 3], label=7)
    alias = cursor
    assert cursor.label == 7 and next(alias) == 1 and list(cursor) == [2]
    assert list(alias) == []

    def reenter(item):
        if item == 1:
            assert next(cursor, None) is None
        return item

    cursor = takewhile(reenter, [1, 0, 3])
    assert next(cursor) == 1 and next(cursor, None) is None
