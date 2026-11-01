from unittest import TestCase


class Unequal:
    def __ne__(self, other):
        return True

    def __eq__(self, other):
        raise RuntimeError("unexpected __eq__")

    def __repr__(self):
        raise RuntimeError("broken repr")


class Equal(Unequal):
    def __ne__(self, other):
        return False

    def __repr__(self):
        return "equal operand"


class Failure(AssertionError):
    pass


def current():
    case = TestCase()
    case.assertNotEqual("self", "__dataclass_self__")
    left = Unequal()
    case.assertNotEqual(left, left)
    case.failureException = Failure
    right = Equal()
    with case.assertRaises(Failure) as caught:
        case.assertNotEqual(right, right, "detail")
    assert str(caught.exception) == "equal operand == equal operand : detail"

    case.longMessage = False
    marker = ["custom message"]
    with case.assertRaises(Failure) as caught:
        case.assertNotEqual("x", "x", marker)
    assert caught.exception.args[0] is marker
