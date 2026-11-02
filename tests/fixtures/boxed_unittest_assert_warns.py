import sys
import unittest
import warnings


def current():
    case = unittest.TestCase()
    with case.assertWarnsRegex(UserWarning, "needle") as caught:
        warnings.warn("other", ResourceWarning)
        warnings.warn("first", UserWarning)
        line = sys._getframe().f_lineno + 1
        warnings.warn("a needle", UserWarning)
    assert str(caught.warning) == "a needle"
    assert (caught.filename, caught.lineno) == (__file__, line)
    assert case.assertWarns(ResourceWarning, warnings.warn, "callback", category=ResourceWarning) is None
    try:
        with case.assertWarns(ResourceWarning):
            warnings.warn("wrong category", UserWarning)
    except AssertionError as error:
        assert str(error) == "ResourceWarning not triggered"
    else:
        raise AssertionError("sibling warning satisfied the assertion")

    class Unnamed:
        __name__ = None

        def __str__(self):
            raise AssertionError("unexpected name conversion")

        def __call__(self, function, *args):
            return function(*args)

    callback = Unnamed()
    case.assertWarns(UserWarning, callback, warnings.warn, "unnamed")
    case.assertRaises(ValueError, callback, int, "invalid")
