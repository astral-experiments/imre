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

    filters = warnings.filters
    assert case._assertNotWarns(UserWarning, warnings.warn, "other", category=ResourceWarning) is None
    with case._assertNotWarns(DeprecationWarning):
        pass
    assert warnings.filters is filters
    try:
        with case._assertNotWarns(Warning, msg="unexpected warning"):
            warnings.warn("matched", UserWarning)
    except case.failureException as error:
        assert str(error) == "Warning triggered : unexpected warning"
    else:
        raise AssertionError("accepted a warning from the requested category")
    sentinel = LookupError("inside the context")
    try:
        with case._assertNotWarns(UserWarning):
            warnings.warn("matched before exception", UserWarning)
            raise sentinel
    except LookupError as error:
        assert error is sentinel
    else:
        raise AssertionError("suppressed an unrelated exception")
    assert warnings.filters is filters
