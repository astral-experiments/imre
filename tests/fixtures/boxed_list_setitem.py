import sys
import unittest
import weakref


class RestorePath(unittest.TestCase):
    def setUp(self):
        self.addCleanup(sys.path.__setitem__, slice(None), list(sys.path))

    def test_change(self):
        sys.path.insert(0, "temporary test path")


class Items(list):
    def __setitem__(self, key, value):
        raise AssertionError("unexpected override")


def current():
    before = sys.path[:]
    result = unittest.TestResult()
    RestorePath("test_change").run(result)
    assert result.testsRun == 1 and result.wasSuccessful()
    assert sys.path == before

    items = Items([1, 2, 3])
    assert list.__setitem__(items, -1, 4) is None
    assert list.__setitem__(items, slice(None, None, 2), [5, 6]) is None
    assert items == [5, 2, 6]
    try:
        list.__setitem__(items, slice(None, None, 2), [7])
    except ValueError:
        pass
    else:
        raise AssertionError("unequal extended slice accepted")
    assert items == [5, 2, 6]

    calls = []

    class IterItems(Items):
        def __iter__(self):
            calls.append("iter")
            return iter([8, 9])

    items = IterItems([1, 2])
    list.__setitem__(items, slice(None, None, -1), items)
    assert items == [2, 1] and calls == []
    list.__setitem__(items, slice(None), IterItems([3, 4]))
    assert items == [8, 9] and calls == ["iter"]
    list.__setitem__(items, 0, items)
    assert items[0] is items

    class FailingHint:
        def __iter__(self):
            calls.append("iter")
            return self

        def __length_hint__(self):
            calls.append("hint")
            raise ValueError("bad hint")

        def __next__(self):
            raise AssertionError("advanced after a failing length hint")

    calls.clear()
    items = [1, 2, 3, 4]
    try:
        list.__setitem__(items, slice(None, None, 2), FailingHint())
    except ValueError as exc:
        assert str(exc) == "bad hint"
    else:
        raise AssertionError("ignored length hint failure")
    assert items == [1, 2, 3, 4] and calls == ["iter", "iter", "hint"]

    # Consuming the replacement can introduce the first weak observer.
    class Element:
        pass

    target = [Element()]
    observers = []

    def empty_replacement():
        observers.append(weakref.ref(target[0]))
        yield from ()

    list.__setitem__(target, slice(None), empty_replacement())
    assert observers[0]() is None
