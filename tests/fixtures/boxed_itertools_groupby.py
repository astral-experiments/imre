from itertools import groupby


def current():
    calls = []

    def key(value):
        calls.append(value)
        return value

    source = [1, 1, 2, 1]
    groups = groupby(source, key=key)
    first_key, first = next(groups)
    assert first_key == 1 and next(first) == 1 and calls == [1]
    second_key, second = next(groups)
    assert second_key == 2 and calls == [1, 1, 2]
    assert list(first) == [] and list(second) == [2]
    last_key, last = next(groups)
    assert last_key == 1 and list(last) == [1] and list(first) == []
    assert list(groups) == [] and calls == source
