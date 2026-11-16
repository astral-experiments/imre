from operator import length_hint


def size_error(cursor):
    try:
        next(cursor)
    except RuntimeError as error:
        assert str(error) == "Set changed size during iteration"
    else:
        raise AssertionError("resized set iterator yielded or exhausted")


def current():
    values = {1}
    cursor = iter(values)
    assert iter(cursor) is cursor
    assert next(cursor) == 1
    values.add(2)
    size_error(cursor)
    values.remove(2)
    size_error(cursor)
    assert length_hint(cursor) == 0

    cursor = iter(values)
    assert next(cursor) == 1
    assert next(cursor, None) is None
    values.add(2)
    assert next(cursor, None) is None

    cursor = iter(values)
    values.add(3)
    assert length_hint(cursor) == 0
    values.remove(1)
    assert length_hint(cursor) == 2
    assert set(cursor) == {2, 3}
    assert set(iter(frozenset(values))) == values
