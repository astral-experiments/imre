from itertools import permutations
from sys import maxsize


def current():
    token = object()
    source = iter([token, 1, 1])
    cursor = permutations(iterable=source, r=2)
    assert next(source, None) is None
    assert iter(cursor) is cursor
    first = next(cursor)
    assert list(cursor) == [(token, 1), (1, token), (1, 1), (1, token), (1, 1)]
    assert first == (token, 1)
    assert next(cursor, None) is None and next(cursor, None) is None

    assert list(permutations([], None)) == [()]
    assert list(permutations("ab", 0)) == [()]
    assert list(permutations("a", 2)) == []
    assert next(permutations(range(32))) == tuple(range(32))

    class IndexOnly:
        def __index__(self):
            raise AssertionError("permutations accepts only int or None")

    for r, error in [(IndexOnly(), TypeError), (-1, ValueError), (maxsize, MemoryError),
                     (maxsize + 1, OverflowError)]:
        source = iter("ab")
        try:
            permutations(source, r)
        except error:
            pass
        else:
            raise AssertionError("invalid selection length")
        assert next(source, None) is None
