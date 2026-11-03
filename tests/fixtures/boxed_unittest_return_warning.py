import unittest
import warnings


def run_case(test):
    test.run()


def debug_case(test):
    test.debug()


def current():
    class ReturnsValue(unittest.TestCase):
        def runTest(self):
            return 1

    class WrappedRun(ReturnsValue):
        def run(self, result=None):
            return super().run(result)

    for invoke, test, origin in (
        (run_case, ReturnsValue(), run_case),
        (debug_case, ReturnsValue(), debug_case),
        (run_case, WrappedRun(), WrappedRun.run),
    ):
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always", DeprecationWarning)
            invoke(test)
        assert len(caught) == 1
        warning = caught[0]
        assert warning.category is DeprecationWarning
        assert warning.filename == origin.__code__.co_filename
        assert warning.lineno == origin.__code__.co_firstlineno + 1

    calls = []

    class WrappedMethod(ReturnsValue):
        def _callTestMethod(self, method):
            calls.append((method.__self__ is self, method.__func__ is ReturnsValue.runTest))
            super()._callTestMethod(method)

    test = WrappedMethod()
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always", DeprecationWarning)
        test.run()
    assert calls == [(True, True)]
    assert len(caught) == 1
    assert caught[0].filename == unittest.TestCase.run.__code__.co_filename
