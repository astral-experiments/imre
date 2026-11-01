import unittest


def current():
    case = unittest.TestCase()
    value = []
    case.assertIs(value, value)
    case.assertIsNot(value, [])
    with case.assertRaises(AssertionError) as different:
        case.assertIs([], value, "identity")
    assert str(different.exception) == "[] is not [] : identity"
    with case.assertRaises(AssertionError) as same:
        case.assertIsNot(value, value)
    assert str(same.exception) == "unexpectedly identical: []"
