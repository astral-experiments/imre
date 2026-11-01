from itertools import combinations, combinations_with_replacement


def current():
    events = []
    item = []

    class Pair:
        def __index__(self):
            events.append("index")
            return 2

    class Source:
        def __iter__(self):
            events.append("iter")
            return iter([item, 2, 2])

    cursor = combinations(Source(), Pair())
    assert events == ["index", "iter"]
    assert iter(cursor) is cursor
    assert next(cursor)[0] is item
    assert list(cursor) == [(item, 2), (2, 2)]
    assert next(cursor, None) is None
    assert list(combinations([1], 2)) == []

    cursor = combinations_with_replacement(iter("ABA"), 2)
    assert next(cursor) == ("A", "A")
    assert list(cursor) == [("A", "B"), ("A", "A"), ("B", "B"), ("B", "A"), ("A", "A")]
    assert next(cursor, None) is None
    assert list(combinations_with_replacement("", 1)) == []

    for make in (combinations, combinations_with_replacement):
        assert list(make(iterable=[], r=0)) == [()]
        # A prefix of an enormous result set only needs one input pool and index vector.
        assert len(next(make(range(80), 40))) == 40
        events.clear()
        try:
            make(Source(), -1)
        except ValueError:
            assert events == ["iter"]
        else:
            raise AssertionError("negative width")
        try:
            make(Source(), 1 << 90)
        except OverflowError:
            assert events == ["iter"]
        else:
            raise AssertionError("width overflow")
