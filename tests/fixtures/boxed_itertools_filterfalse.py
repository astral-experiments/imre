from itertools import filterfalse


def current():
    calls = []

    class Source:
        def __iter__(self):
            calls.append("iter")
            return iter([1, 2, 3, 4])

    def predicate(value):
        calls.append(value)
        if value == 2:
            raise ValueError("two")
        return value % 2

    try:
        filterfalse.__new__(int, None, Source())
    except TypeError:
        pass
    else:
        raise AssertionError("constructed an unrelated type")
    assert not calls

    cursor = filterfalse(predicate, Source())
    assert iter(cursor) is cursor and calls == ["iter"]
    try:
        next(cursor)
    except ValueError:
        pass
    else:
        raise AssertionError("predicate error lost")
    assert calls == ["iter", 1, 2]
    assert next(cursor) == 4 and next(cursor, None) is None
    assert list(filterfalse(None, [0, 1, [], False])) == [0, [], False]
    assert list(filterfalse(bool, [0, 1])) == [0]
    assert list(filterfalse({1, 3}.__contains__, [3, 2, 1])) == [2]

    class Cursor(filterfalse):
        @property
        def _iterator(self):
            raise AssertionError("read subclass attribute")

        def __init__(self, *args, label=None):
            self.label = label

    cursor = Cursor(None, [0], label="subclass")
    assert type(cursor) is Cursor and cursor.label == "subclass" and list(cursor) == [0]

    import pickle
    for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
        try:
            pickle.dumps(filterfalse(None, [0]), protocol)
        except TypeError as error:
            name = "filterfalse" if protocol < 2 else "itertools.filterfalse"
            assert str(error) == f"cannot pickle '{name}' object"
        else:
            raise AssertionError("pickled filterfalse")
