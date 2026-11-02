def current():
    import unittest
    import warnings
    from warnings import warn

    class Case(unittest.TestCase):
        class failureException(AssertionError):
            pass

    class Notice(UserWarning):
        pass

    def emit(message):
        warn(message)

    case = Case()
    first = Notice("first")
    with warnings.catch_warnings(record=True) as outer:
        warnings.simplefilter("error", UserWarning)
        with case.assertWarns((UserWarning, FutureWarning)) as ctx:
            warn("other category", RuntimeWarning)
            emit(first)
            warn("second", UserWarning)
        assert ctx.warning is first
        assert ctx.filename == emit.__code__.co_filename
        assert ctx.lineno == emit.__code__.co_firstlineno + 1
        with case.assertRaises(UserWarning):
            warn("restored filter")
        assert not outer

    def callback(category, *, msg):
        warn(msg, category)
        return "ignored"

    assert case.assertWarns(Warning, callback, RuntimeWarning, msg="forwarded") is None
    with case.assertRaises(ValueError) as caught:
        with case.assertWarns(UserWarning):
            raise ValueError("uncaught")
    assert caught.exception.args == ("uncaught",)

    with case.assertRaises(Case.failureException) as caught:
        with case.assertWarns(UserWarning, msg="expected one"):
            warn("unrelated", RuntimeWarning)
    assert str(caught.exception) == "UserWarning not triggered : expected one"
    with case.assertRaises(Case.failureException) as caught:
        case.assertWarns(UserWarning, callback, RuntimeWarning, msg="unrelated")
    assert str(caught.exception) == "UserWarning not triggered by callback"

    class Silent:
        def __call__(self):
            pass

    silent = Silent()
    for name, suffix in ((None, ""), (42, " by 42")):
        silent.__name__ = name
        with case.assertRaises(Case.failureException) as caught:
            case.assertWarns(UserWarning, silent)
        assert str(caught.exception) == "UserWarning not triggered" + suffix

    for expected in (ValueError, (UserWarning, 1)):
        with case.assertRaises(TypeError) as caught:
            case.assertWarns(expected)
        assert str(caught.exception) == "assertWarns() arg 1 must be a warning type or tuple of warning types"
    with case.assertRaises(TypeError) as caught:
        case.assertWarns(Warning, unexpected=1)
    assert str(caught.exception) == "'unexpected' is an invalid keyword argument for this function"
