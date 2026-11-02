"""A module example that changes only its own globals.

>>> retained = sys.stdout
>>> print("same capture", file=retained)
same capture
>>> seed = 42
>>> seed
42
"""
import doctest
import sys
import unittest


class Examples:
    @staticmethod
    def nested():
        """
        >>> seed
        40
        """


__test__ = {"extra": ">>> print('captured')\ncaptured\n"}


def current():
    module = sys.modules[__name__]
    shadow = type(module)(__name__)
    shadow.__file__ = __file__
    shadow.Examples = Examples
    assert not list(doctest.DocTestSuite(shadow))
    suite = doctest.DocTestSuite(module, extraglobs={"seed": 40})
    assert type(suite) is doctest._DocTestSuite and isinstance(suite, unittest.TestSuite)
    assert [case.id() for case in suite] == [__name__, __name__ + ".Examples.nested", __name__ + ".__test__.extra"]
    globs = next(iter(suite))._dt_test.globs
    globs["seed"] = 41
    for _ in range(2):
        result = unittest.TestResult()
        suite.run(result)
        assert result.testsRun == 3 and result.wasSuccessful(), result.errors + result.failures
        assert next(iter(suite))._dt_test.globs is globs and globs["seed"] == 41

    __test__["extra"] = ">>> 6 * 7\n41\n"
    try:
        result = unittest.TestResult()
        doctest.DocTestSuite(module, extraglobs={"seed": 40}).run(result)
        assert not result.errors and len(result.failures) == 1
        case, message = result.failures[0]
        assert case.id() == __name__ + ".__test__.extra" and "41" in message and "42" in message
    finally:
        __test__["extra"] = ">>> print('captured')\ncaptured\n"
