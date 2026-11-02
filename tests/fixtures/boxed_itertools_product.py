from itertools import product


def current():
    events = []
    item = []

    class Pool:
        def __iter__(self):
            events.append("pool")
            return iter([item, 1])

    class Twice:
        def __index__(self):
            events.append("repeat")
            return 2

    cursor = product(Pool(), repeat=Twice())
    assert events == ["repeat", "pool"]
    alias = iter(cursor)
    saved = next(cursor)
    assert alias is cursor and saved[0] is saved[1] is item
    assert next(alias) == (item, 1)
    assert list(cursor) == [(1, item), (1, 1)]
    assert saved == (item, item) and next(alias, None) is None
    assert list(product()) == [()] and list(product(None, repeat=0)) == [()]

    assert next(product()) is next(product(None, repeat=0))

    class LabelledProduct(product):
        def __next__(self):
            return super().__next__() + ("label",)

    assert list(LabelledProduct(range(2), [item], "ab")) == [
        (0, item, "a", "label"), (0, item, "b", "label"),
        (1, item, "a", "label"), (1, item, "b", "label"),
    ]

    class Broken:
        def __iter__(self):
            raise RuntimeError("opening pool")

    try:
        product([], Broken())
    except RuntimeError as error:
        assert str(error) == "opening pool"
    else:
        raise AssertionError("did not open all pools")

    assert list(product("abc", [])) == []
    assert next(product(range(2), repeat=60)) == (0,) * 60
    for repeat, exc in [(-1, ValueError), (1.5, TypeError), (1 << 80, OverflowError)]:
        try:
            product(Broken(), repeat=repeat)
        except exc:
            pass
        else:
            raise AssertionError("invalid repeat")
