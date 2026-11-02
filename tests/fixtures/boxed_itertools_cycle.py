import itertools
from itertools import *

assert cycle is itertools.cycle and count is itertools.count and repeat is itertools.repeat
assert "_cycle_next" not in globals()


def current():
    class Source:
        def __init__(self):
            self.calls = 0

        def __iter__(self):
            self.calls += 1
            return self

        def __next__(self):
            self.calls += 1
            if self.calls == 2:
                raise ValueError
            if self.calls == 3:
                return marker
            raise StopIteration

    class Tagged(cycle):
        def __init__(self, iterable, *, tag):
            self.tag = tag

    marker = object()
    source = Source()
    cursor = Tagged(source, tag=marker)
    assert cursor.tag is marker and iter(cursor) is cursor and source.calls == 1
    try:
        next(cursor)
    except ValueError:
        pass
    else:
        raise AssertionError("cycle swallowed a source error")
    assert next(cursor) is marker and next(cursor) is marker and source.calls == 4
    assert next(cursor) is marker and source.calls == 4
    class Guarded(cycle):
        def __setattr__(self, name, value):
            raise AssertionError(name)

    cursor = Guarded([marker])
    assert next(cursor) is next(cursor) is marker
    assert not any(hasattr(cursor, name) for name in ("_source", "_saved", "_index"))
    tagged = Tagged([marker], tag=marker)
    tagged._index = "my field"
    assert next(tagged) is next(tagged) is marker and tagged._index == "my field"

    class Reducer(cycle):
        def __reduce__(self):
            return (str, ("custom",))

    for protocol in (0, 2, 5):
        assert Reducer([]).__reduce_ex__(protocol) == (str, ("custom",))

    class Args(cycle):
        def __getnewargs__(self):
            return (("restored",),)

    class State(cycle):
        def __getstate__(self):
            return {"custom": 7}

    for item, args, state in ((Args([]), (("restored",),), None), (State([]), (), {"custom": 7})):
        factory, saved_args, data, items, entries = item.__reduce_ex__(5)
        assert factory.__name__ == "__newobj__" and saved_args == (type(item), *args)
        assert data == state and items is None and entries is None
    assert list(cycle(())) == []
    return "ok"
