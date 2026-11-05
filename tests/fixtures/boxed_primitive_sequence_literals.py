import gc


def literals():
    return [None, False, 0, True, 1, 255, 256, ..., 0xff]


def current():
    left, right = literals(), literals()
    left[0] = 7
    gc.collect()
    assert left is not right and right[0] is None
    values = (None, False, 0, True, 1, 255, 256, ..., 0xff)
    assert all(x is y for x, y in zip(right, values))
    calls = []

    def value(n):
        calls.append(n)
        return n

    assert [1, value(2), *[3, 4], value(5)] == [1, 2, 3, 4, 5]
    assert calls == [2, 5]
    assert [0, 257, -6, True] == list((0, 257, -6, True))
    print(right, values)
