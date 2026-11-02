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

    assert case.assertIsSubclass(bool, (str, int)) is None
    assert case.assertNotIsSubclass(str, int) is None
    with case.assertRaises(Failure) as caught:
        case.assertIsSubclass(str, (bool, int), "detail")
    assert str(caught.exception) == "<class 'str'> is not a subclass of any of (<class 'bool'>, <class 'int'>) : detail"
    with case.assertRaises(Failure) as caught:
        case.assertNotIsSubclass(bool, (str, int, bool))
    assert str(caught.exception) == "<class 'bool'> is a subclass of <class 'int'>"
    with case.assertRaises(Failure) as caught:
        case.assertIsSubclass(42, int)
    assert str(caught.exception) == "42 is not a class"
    with case.assertRaises(TypeError):
        case.assertNotIsSubclass(int, 42)

    case.longMessage = False
    marker = ["custom message"]
    with case.assertRaises(Failure) as caught:
        case.assertNotIsInstance(1, int, marker)
    assert caught.exception.args[0] is marker
