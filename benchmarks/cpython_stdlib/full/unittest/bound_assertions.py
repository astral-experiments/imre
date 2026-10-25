"""Check receiver identity, argument evaluation and real failures through stored assertions."""

import unittest


class MyFailure(AssertionError):
    pass


class Subject(unittest.TestCase):
    failureException = MyFailure


class ReturningFail(Subject):
    def fail(self, msg: str | None = None) -> None:
        print("fail hook")


def receiver(case: unittest.TestCase) -> unittest.TestCase:
    print("bind")
    return case


def argument(text: str) -> str:
    print(text)
    return text


def fail_with(text: str) -> None:
    raise ValueError(text)


def success() -> None:
    pass


def saved_equal() -> None:
    subject = Subject()
    alias = subject
    equal = receiver(subject).assertEqual
    seq_equal = subject.assertListEqual
    subject = Subject()
    alias.longMessage = False
    equal("same", "same")
    equal(["first", "second"], ["first", "second"])
    seq_equal([b"data"], [b"data"])
    try:
        equal(argument("left"), argument("right"), msg="saved case")
    except MyFailure as e:
        print(str(e))
    alias.longMessage = True
    try:
        equal(2, 5, "changed case")
    except MyFailure as e:
        print(str(e))
    sequence = [seq_equal]
    try:
        sequence[0](["old"], ["new"])
    except MyFailure as e:
        print(str(e))


def saved_raises() -> None:
    subject = Subject()
    raises = receiver(subject).assertRaises
    print(raises(ValueError, fail_with, "expected") is None)
    with raises(LookupError) as cm:
        raise KeyError("original")
    print(str(cm.exception))
    try:
        raises(ValueError, success)
    except MyFailure as e:
        print(str(e))
    equal = subject.assertEqual
    print(raises(MyFailure, equal, "a", "b") is None)
    try:
        raises(MyFailure, equal, "a", "a")
    except MyFailure as e:
        print(str(e))


def equality_failure_dispatch() -> None:
    case = ReturningFail()
    equal = case.assertEqual
    with case.assertRaises(MyFailure):
        equal(1, 2)
    with case.assertRaises(MyFailure):
        case.assertNotEqual(1, 1)
    case._diffThreshold = 2
    with case.assertRaises(MyFailure):
        equal("ab", "abc")
    equal("ab", "ac")
    case.assertLess(2, 1)


saved_equal()
saved_raises()
equality_failure_dispatch()
