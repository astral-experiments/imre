from unittest import TestCase


class Failure(AssertionError):
    pass


class Case(TestCase):
    failureException = Failure


def current():
    case = Case()
    assert case.assertIsInstance(True, int) is None
    assert case.assertNotIsInstance(None, (int, str)) is None

    with case.assertRaises(Failure) as caught:
        case.assertIsInstance(1, (str, list), msg="detail")
    assert str(caught.exception) == "1 is not an instance of any of (<class 'str'>, <class 'list'>) : detail"

    with case.assertRaises(Failure) as caught:
        case.assertNotIsInstance(True, (str, int, bool))
    assert str(caught.exception) == "True is an instance of <class 'int'>"

    case.longMessage = False
    marker = ["custom message"]
    with case.assertRaises(Failure) as caught:
        case.assertNotIsInstance(1, int, marker)
    assert caught.exception.args[0] is marker
