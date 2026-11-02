from unittest import TestCase


class Failure(AssertionError):
    pass


class Case(TestCase):
    failureException = Failure


class Tail:
    def __init__(self):
        self.calls = []

    def endswith(self, suffix):
        self.calls.append(suffix)
        return suffix == "match"

    def __repr__(self):
        return "<tail>"


def current():
    case = Case()
    case.assertStartsWith("filename", ("bad", "file", 3))
    case.assertStartsWith(bytearray(b"filename"), (b"bad", bytearray(b"file")))
    for value, prefix, expected in (
        ("filename", "name", "'filename' doesn't start with 'name'"),
        ("abc", ("x", "y"), "'abc' doesn't start with any of ('x', 'y')"),
        (b"abc", "a", "Expected str, not bytes"),
    ):
        try:
            case.assertStartsWith(value, prefix, msg="extra")
        except Failure as error:
            assert str(error) == expected + " : extra", str(error)
        else:
            raise AssertionError("prefix mismatch succeeded")
    tail = Tail()
    case.assertEndsWith(tail, "match")
    case.assertEndsWith("filename", ("bad", "name", 3))
    case.assertEndsWith(bytearray(b"filename"), (b"bad", bytearray(b"name")))
    case.assertEndsWith(b"filename", bytearray(b"name"))
    for value, suffix, expected in (
        (tail, "miss", "<tail> doesn't end with 'miss'"),
        ("abc", ("x", "y"), "'abc' doesn't end with any of ('x', 'y')"),
        (b"abc", "c", "Expected str, not bytes"),
        ("abc", (b"c",), "Expected bytes, not str"),
        ([], "c", "Expected str, not list"),
        ("x" * 82, "z", "'" + "x" * 79 + " [truncated]... doesn't end with 'z'"),
    ):
        try:
            case.assertEndsWith(value, suffix, msg="extra")
        except Failure as error:
            assert str(error) == expected + " : extra", str(error)
        else:
            raise AssertionError("suffix mismatch succeeded")
    assert tail.calls == ["match", "miss"]
    case.longMessage = False
    try:
        case.assertEndsWith("abc", "x", "override")
    except Failure as error:
        assert str(error) == "override"
    else:
        raise AssertionError("longMessage=False suppressed failure")
    try:
        case.assertEndsWith("abc", 4)
    except TypeError as error:
        assert str(error) == "endswith first arg must be str or a tuple of str, not int"
    else:
        raise AssertionError("invalid suffix was accepted")
    print("suffix assertions passed")
