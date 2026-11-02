from unittest import TestCase


class Ordered:
    def __lt__(self, other):
        return other == 7

    def __le__(self, other):
        return other is self


def current():
    case = TestCase()
    case.assertLessEqual(1.25, 1.25)
    item = Ordered()
    case.assertLessEqual(item, item)
    case.assertLess(item, 7)
    case.assertGreater(7, item)
    case.assertGreaterEqual(1.25, 1.25)
    with case.assertRaises(AssertionError) as caught:
        case.assertLessEqual(1.5, 1.25, "timestamp")
    assert str(caught.exception) == "1.5 not less than or equal to 1.25 : timestamp"
