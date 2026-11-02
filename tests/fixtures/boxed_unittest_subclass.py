from unittest import TestCase


class Failure(AssertionError):
    pass


class Case(TestCase):
    failureException = Failure


def current():
    case = Case()
    assert case.assertIsSubclass(bool, int) is None
    assert case.assertNotIsSubclass(int, (str, list)) is None

    with case.assertRaises(Failure) as caught:
        case.assertIsSubclass(int, (str, list), msg="detail")
    assert str(caught.exception) == "<class 'int'> is not a subclass of any of (<class 'str'>, <class 'list'>) : detail"

    with case.assertRaises(Failure) as caught:
        case.assertNotIsSubclass(bool, (str, int, bool))
    assert str(caught.exception) == "<class 'bool'> is a subclass of <class 'int'>"

    for check in (case.assertIsSubclass, case.assertNotIsSubclass):
        with case.assertRaises(Failure) as caught:
            check(42, int)
        assert str(caught.exception) == "42 is not a class"
        with case.assertRaises(TypeError):
            check(int, 42)
