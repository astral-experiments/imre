import re
from unittest import TestCase


class Failure(Exception):
    pass


class Case(TestCase):
    failureException = Failure


def current():
    case = Case()
    assert case.assertRegex("abc", "b") is None
    assert case.assertRegex(b"abc", b"b") is None
    assert case.assertRegex(b"abc", re.compile(b"B", re.I)) is None
    assert case.assertRegex("", re.compile("")) is None

    with case.assertRaises(AssertionError) as caught:
        case.assertRegex(b"abc", b"")
    assert str(caught.exception) == "expected_regex must not be empty."

    with case.assertRaises(Failure) as caught:
        case.assertRegex(b"abc", re.compile(b"x"), "detail")
    assert str(caught.exception) == "Regex didn't match: b'x' not found in b'abc' : detail"

    case.longMessage = False
    with case.assertRaises(Failure) as caught:
        case.assertRegex("abc", "x", msg="detail")
    assert str(caught.exception) == "detail"

    case.longMessage = True
    assert case.assertNotRegex("abc", "x") is None
    with case.assertRaises(Failure) as caught:
        case.assertNotRegex(b"one two", re.compile(b"T.O", re.I), msg="detail")
    assert str(caught.exception) == "Regex matched: b'two' matches b'T.O' in b'one two' : detail"
    with case.assertRaises(Failure) as caught:
        case.assertNotRegex("abc", "")
    assert str(caught.exception) == "Regex matched: '' matches '' in 'abc'"
    with case.assertRaises(TypeError):
        case.assertNotRegex(b"abc", "a")
