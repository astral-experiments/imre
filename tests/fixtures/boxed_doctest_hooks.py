import doctest
from functools import wraps
import sys
import unittest


def decorate(fn):
    @wraps(fn)
    def wrapped():
        return fn()
    return wrapped


def source_backed():
    # The docstring starts below the definition.
    """A failing example in this file.

    >>> 1 + 1
    3
    """


source_backed = decorate(source_backed)


__test__ = {"string": "A standalone string.\n\n>>> 1 + 1\n3\n"}


def current():
    class DisplayError(Exception):
        __module__ = "plugins.errors"

        def __str__(self):
            type(self).__module__ = "builtins"
            return "friendly"

    doc = f">>> raise problem\nTraceback (most recent call last):\n...\n{DisplayError.__qualname__}: friendly\n"
    test = doctest.DocTestParser().get_doctest(doc, {"problem": DisplayError()}, "guide", "guide.txt", 0)
    doctest.DocTestCase(test).runTest()

    class Event(Exception):
        __module__ = None

    module = type(sys)(__name__ + "_hooks")
    module.__doc__ = (
        "A module with a mutable example namespace.\n\n"
        ">>> events.append(seed)\n>>> print(seed)\nexam...\n"
        ">>> raise Event('detail')  # doctest: +IGNORE_EXCEPTION_DETAIL\n"
        "Traceback (most recent call last):\n...\nEvent: expected\n"
    )
    events = []
    marker = ValueError("hook")
    phase = ""

    def setup(test):
        assert test is case._dt_test and test.globs is globs
        events.append("setup")
        test.globs.update(seed="example", events=events, marker=marker, Event=Event)
        if phase == "setup":
            raise marker

    def teardown(test):
        assert test is case._dt_test and test.globs is globs
        events.append("teardown")
        if phase == "teardown":
            raise marker

    for phase, order in (
        ("", ["setup", "example", "teardown", "clean"]),
        ("setup", ["setup", "clean"]),
        ("teardown", ["setup", "example", "teardown", "clean"]),
    ):
        events.clear()
        (case,) = doctest.DocTestSuite(module, setUp=setup, tearDown=teardown, optionflags=doctest.ELLIPSIS)
        globs = case._dt_test.globs
        case.addCleanup(events.append, "clean")
        result = unittest.TestResult()
        case.run(result)
        assert result.testsRun == 1 and not result.failures
        assert len(result.errors) == bool(phase) and events == order
        assert case._dt_test.globs is globs and ("seed" in globs) == bool(phase)
        if phase:
            try:
                case.debug()
            except ValueError as exc:
                assert exc is marker
            else:
                raise AssertionError("missing hook exception")

    phase = ""
    events.clear()
    module.__doc__ = "Unexpected exception.\n\n>>> raise marker\n"
    (case,) = doctest.DocTestSuite(module, setUp=setup, tearDown=teardown)
    globs = case._dt_test.globs
    try:
        case.debug()
    except doctest.UnexpectedException as exc:
        assert exc.test is case._dt_test and exc.example is exc.test.examples[0]
        assert exc.exc_info[1] is marker and events == ["setup"]
        if sys.implementation.name == "imre":
            assert exc.args == (str(exc),) and "Failed example:" in str(exc)
    else:
        raise AssertionError("missing example exception")

    for filename, first_line, location in (
        (None, None, "Line 3, in sample"),
        ("sample.py", None, 'File "sample.py", line ?, in sample'),
        ("sample.py", 40, 'File "sample.py", line 43, in sample'),
    ):
        test = doctest.DocTestParser().get_doctest(
            "A failing example.\n\n>>> 1 + 1\n3\n", {}, "sample", filename, first_line
        )
        result = unittest.TestResult()
        doctest.DocTestCase(test).run(result)
        assert len(result.failures) == 1 and not result.errors
        assert location + "\nFailed example:" in result.failures[0][1], result.failures
    string_case, source_case = doctest.DocTestSuite(sys.modules[__name__])
    source_line = source_backed.__wrapped__.__code__.co_firstlineno
    for case, origin, line in ((string_case, None, "?"), (source_case, source_line + 1, source_line + 4)):
        test = case._dt_test
        assert test.filename == __file__ and test.lineno == origin and test.examples[0].lineno == 2
        result = unittest.TestResult()
        case.run(result)
        assert len(result.failures) == 1 and not result.errors
        assert f'File "{__file__}", line {line}, in {test.name}\nFailed example:' in result.failures[0][1]
    print("boxed doctest hooks")
