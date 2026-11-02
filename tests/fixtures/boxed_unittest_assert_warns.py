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
