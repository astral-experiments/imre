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

    assert case.assertNotRegex("abc", "z") is None
    case.longMessage = True
    with case.assertRaises(Failure) as caught:
        case.assertNotRegex(b"abc", re.compile(b"B", re.I), "detail")
    assert str(caught.exception) == "Regex matched: b'b' matches b'B' in b'abc' : detail"
