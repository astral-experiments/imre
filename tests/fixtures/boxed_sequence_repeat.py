def current():
    marker = object()
    items = []
    calls = []

    class Count:
        def __index__(self):
            calls.append(1)
            items.append(marker)
            return 2

    count = Count()
    assert items * count == [marker, marker]
    assert count * (marker,) == (marker, marker)
    original = items
    items *= count
    assert items is original and items == [marker] * 6
    assert len(calls) == 3

    class Pair(tuple):
        pass

    pair = Pair((1, 2))
    for result in (pair * 1, 1 * pair, pair * 0, pair * 2, Pair(()) * 1):
        assert type(result) is tuple
    assert pair * 1 == (1, 2) and pair * 1 is not pair * 1
    assert 1 * pair is not 1 * pair
    assert pair * 0 == () and pair * 2 == (1, 2, 1, 2)

    class Reflected:
        def __rmul__(self, other):
            return marker

    items *= Reflected()
    assert items is marker

    class Buffer(bytearray):
        pass

    data = Buffer(b"ab")
    clone = data * 1
    assert type(clone) is bytearray and clone is not data and clone == b"ab"
    assert 2 * data == b"abab"
    alias = data
    data *= 3
    assert data is alias and data == b"ababab"
    data *= -1
    assert data is alias and data == b""
    print("repeated after index conversion")
