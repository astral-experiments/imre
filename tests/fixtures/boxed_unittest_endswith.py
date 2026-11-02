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
    not_ends = case.assertNotEndsWith
    assert not_ends("prefix:end", "prefix") is None
    assert not_ends("prefix:end", ()) is None
    assert not_ends(bytearray(b"prefix:end"), (b"prefix", b"miss")) is None
    assert case.assertNotStartsWith("prefix:end", "end") is None
    for check, value, affix, expected in (
        (not_ends, tail, "match", "<tail> ends with 'match'"),
        (not_ends, "abc", ("x", "bc", "c", 3), "'abc' ends with 'bc'"),
        (not_ends, b"abc", "c", "Expected str, not bytes"),
        (not_ends, "abc", (b"c",), "Expected bytes, not str"),
        (not_ends, [], "c", "Expected str, not list"),
        (case.assertNotStartsWith, "abc", ("x", "ab", "a"), "'abc' starts with 'ab'"),
    ):
        with case.assertRaises(Failure) as caught:
            check(value, affix, msg="detail")
        assert str(caught.exception) == expected + " : detail", str(caught.exception)
    with case.assertRaises(TypeError) as caught:
        not_ends("abc", 4)
    assert str(caught.exception) == "endswith first arg must be str or a tuple of str, not int"

    case.longMessage = False
    marker = ["custom message"]
    with case.assertRaises(Failure) as caught:
        not_ends("abc", "c", marker)
    assert caught.exception.args[0] is marker
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
