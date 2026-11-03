import unittest


def current():
    case = unittest.TestCase()
    compare = case.assertSetEqual
    calls = []

    class Difference:
        def __init__(self, result):
            self.result = result

        def difference(self, other):
            calls.append((self, other))
            return self.result

    first = Difference(["one"])
    second = Difference(["two"])
    with case.assertRaises(AssertionError) as caught:
        compare(first, second, msg="detail")
    assert calls == [(first, second), (second, first)]
    assert str(caught.exception) == (
        "Items in the first set but not the second:\n'one'\n"
        "Items in the second set but not the first:\n'two' : detail"
    )

    class Routed(unittest.TestCase):
        def assertSetEqual(self, first, second, *, msg):
            calls.append(msg)
            return "override result"

    class Other(set):
        pass

    calls.clear()
    routed = Routed()
    assert routed.assertEqual({0}, {1}, "set") is None
    assert routed.assertEqual(frozenset({0}), frozenset({1}), "frozen") is None
    routed.assertEqual({0}, frozenset({0}), "mixed")
    routed.assertEqual(Other([0]), Other([0]), "subclass")
    assert calls == ["set", "frozen"]

    def compare(first, second, *, msg):
        calls.append((first, second, msg))
        return False

    first, second = Other([0]), Other([1])
    routed.addTypeEqualityFunc(Other, compare)
    assert routed.assertEqual(first, second, "custom") is None
    assert calls[-1] == (first, second, "custom")
    with case.assertRaises(AssertionError):
        case.assertEqual(first, second)

    routed.addTypeEqualityFunc(Other, "assertSetEqual")
    routed.assertEqual(first, second)
    assert calls[-1] is None
