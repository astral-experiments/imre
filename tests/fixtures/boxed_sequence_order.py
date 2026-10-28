def current():
    result = object()
    trace = []

    class Item:
        def __init__(self, side):
            self.side = side

        def __eq__(self, other):
            trace.append((self.side, "eq"))
            return False

        def __lt__(self, other):
            trace.append((self.side, "lt"))
            return result

        def __gt__(self, other):
            trace.append((self.side, "gt"))
            return result

    for base in (list, tuple):
        class Child(base):
            pass

        left = base([0, Item("left")])
        right = Child([0, Item("right")])
        trace.clear()
        assert (left < right) is result
        assert trace == [("right", "eq"), ("right", "gt")]
        trace.clear()
        assert (right < left) is result
        assert trace == [("right", "eq"), ("right", "lt")]

    class Replace:
        def __eq__(self, other):
            a[0] = 9
            return False

    a = [Replace()]
    assert (a < [7]) is False
    left = []
    right = []
    left.append(left)
    right.extend([right, 0])
    try:
        left < right
    except RecursionError:
        pass
    else:
        raise AssertionError("recursive ordering")
    # Tuple keys can run Python comparisons while the JSON encoder owns a sorted snapshot.
    import json

    calls = []

    class Part:
        def __init__(self, value):
            self.value = value

        def __hash__(self):
            return self.value

        def __eq__(self, other):
            calls.append(self.value)
            source.clear()
            if fail:
                raise ValueError("stop sorting")
            for i in range(3900):
                cycle = [i]
                cycle.append(cycle)
            return True

    for fail, expected in ((True, ValueError), (False, TypeError)):
        calls.clear()
        source = {(Part(i), i): i for i in (4, 1, 3, 2)}
        try:
            json.dumps(source, sort_keys=True)
        except Exception as exc:
            assert type(exc) is expected
        else:
            raise AssertionError("tuple keys cannot be encoded")
        assert not source
        if fail:
            assert len(calls) == 1
    print("compared the first unequal elements")
