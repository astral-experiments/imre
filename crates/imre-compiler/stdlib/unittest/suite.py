"""Base and fixture-aware suites for the boxed unittest package."""

from unittest import TestCase, _suiteDebug, _suiteRun
from unittest.util import strclass

__unittest = True


# Adapted from CPython 3.14.5 Lib/unittest/suite.py. See licenses/CPython.txt.
class BaseTestSuite:
    """A test suite that runs its members without class or module fixtures."""

    _cleanup = True

    def __init__(self, tests=()):
        self._tests = []
        self._removed_tests = 0
        self.addTests(tests)

    def __repr__(self):
        return "<%s tests=%s>" % (strclass(self.__class__), list(self))

    def __eq__(self, other):
        if not isinstance(other, self.__class__):
            return NotImplemented
        return list(self) == list(other)

    def __iter__(self):
        return iter(self._tests)

    def countTestCases(self):
        count = self._removed_tests
        for test in self:
            if test:
                count += test.countTestCases()
        return count

    def addTest(self, test):
        if not callable(test):
            raise TypeError(repr(test) + " is not callable")
        if isinstance(test, type) and issubclass(test, (TestCase, TestSuite)):
            raise TypeError("TestCases and TestSuites must be instantiated before passing them to addTest()")
        self._tests.append(test)

    def addTests(self, tests):
        if isinstance(tests, str):
            raise TypeError("tests must be an iterable of tests, not a string")
        for test in tests:
            self.addTest(test)

    def run(self, result):
        for index, test in enumerate(self):
            if result.shouldStop:
                break
            test(result)
            if self._cleanup:
                self._removeTestAtIndex(index)
        return result

    def _removeTestAtIndex(self, index):
        try:
            test = self._tests[index]
        except TypeError:
            return
        if hasattr(test, "countTestCases"):
            self._removed_tests += test.countTestCases()
        self._tests[index] = None

    def __call__(self, *args, **kwargs):
        return self.run(*args, **kwargs)

    def debug(self):
        for test in self:
            test.debug()


class TestSuite(BaseTestSuite):
    """A suite that handles class and module fixtures for its tests."""

    run = _suiteRun
    debug = _suiteDebug
