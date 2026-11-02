from itertools import batched
from sys import maxsize


def current():
    events = []

    class Size:
        def __index__(self):
            events.append("index")
            return 2

    class Strict:
        def __bool__(self):
            events.append("strict")
            return False

    class Source:
        def __iter__(self):
            events.append("iter")
            return iter([1, 2, 3])

    class Batches(batched):
        @property
        def _batched_state(self):
            return "subclass value"

        def __init__(self, *args, **kwargs):
            events.append("init")

    cursor = Batches(iterable=Source(), n=Size(), strict=Strict())
    assert events == ["index", "strict", "iter", "init"]
    assert cursor._batched_state == "subclass value"
    assert type(cursor) is Batches
    assert iter(cursor) is cursor
    assert next(cursor) == (1, 2)
    batched.__init__(cursor)
    assert next(cursor) == (3,)
    assert next(cursor, None) is None
    base = batched("xy", 1)
    assert not hasattr(base, "_batched_state")
    assert next(base) == ("x",)
    batched.__init__(base, "other", 1)
    assert next(base) == ("y",)
    try:
        batched.__init__(cursor, "other", 1)
    except TypeError:
        pass
    else:
        raise AssertionError("subclass defines its own initializer")
    assert list(batched.__new__(Batches, "abcd", 2)) == [("a", "b"), ("c", "d")]
    assert "for batch in batched('ABCDEFG', 3):" in batched.__doc__

    cursor = batched("abc", 2, strict=True)
    assert next(cursor) == ("a", "b")
    try:
        next(cursor)
    except ValueError as exc:
        assert str(exc) == "batched(): incomplete batch"
    else:
        raise AssertionError("incomplete batch")
    assert next(cursor, None) is None

    source = iter([1])
    cursor = batched(source, maxsize)
    try:
        next(cursor)
    except MemoryError:
        assert next(source) == 1
    else:
        raise AssertionError("unallocatable batch")

    class Faulty:
        def __iter__(self):
            return self

        def __next__(self):
            events.append("next")
            raise KeyError("source")

    cursor = batched(Faulty(), 2)
    try:
        next(cursor)
    except KeyError as exc:
        assert exc.args == ("source",)
    else:
        raise AssertionError("source exception")
    assert next(cursor, None) is None
    assert events.count("next") == 1
