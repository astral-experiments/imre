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

    class Reflected:
        def __rmul__(self, other):
            return marker

    items *= Reflected()
    assert items is marker
    print("repeated after index conversion")
