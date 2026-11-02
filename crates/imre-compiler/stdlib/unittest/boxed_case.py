import contextlib
import warnings
from unittest import SkipTest, TestCase, _enterCleanupContext

_module_cleanups = []


def addModuleCleanup(function, /, *args, **kwargs):
    _module_cleanups.append((function, args, kwargs))


def enterModuleContext(cm):
    return _enterCleanupContext(cm, addModuleCleanup)


def doModuleCleanups():
    errors = []
    while _module_cleanups:
        function, args, kwargs = _module_cleanups.pop()
        try:
            function(*args, **kwargs)
        except Exception as exc:
            errors.append(exc)
    if errors:
        raise errors[0]


# CPython 3.14.5 Lib/unittest/case.py. See stdlib/docstrings/LICENSE.
class _ShouldStop(Exception):
    """
    The test should stop.
    """


class _Outcome(object):
    def __init__(self, result=None):
        self.expecting_failure = False
        self.result = result
        self.result_supports_subtests = hasattr(result, "addSubTest")
        self.success = True
        self.expectedFailure = None

    def testPartExecutor(self, test_case, subTest=False):
        return _TestPartExecutor(self, test_case, subTest)


class _TestPartExecutor(contextlib.ContextDecorator):
    def __init__(self, outcome, test_case, subtest):
        self.outcome = outcome
        self.test_case = test_case
        self.subtest = subtest

    def _recreate_cm(self):
        return _TestPartExecutor(self.outcome, self.test_case, self.subtest)

    def __enter__(self):
        self.old_success = self.outcome.success
        self.outcome.success = True

    def __exit__(self, typ, value, tb):
        outcome = self.outcome
        test_case = self.test_case
        try:
            if typ is None:
                if self.subtest and outcome.success:
                    outcome.result.addSubTest(test_case.test_case, test_case, None)
            elif issubclass(typ, KeyboardInterrupt):
                return False
            elif issubclass(typ, SkipTest):
                outcome.success = False
                _addSkip(outcome.result, test_case, str(value))
            elif not issubclass(typ, _ShouldStop):
                info = (typ, value, tb)
                if outcome.expecting_failure:
                    outcome.expectedFailure = info
                else:
                    outcome.success = False
                    if self.subtest:
                        outcome.result.addSubTest(test_case.test_case, test_case, info)
                    else:
                        _addError(outcome.result, test_case, info)
            return typ is not None
        finally:
            outcome.success = outcome.success and self.old_success


def _addSkip(result, test_case, reason):
    addSkip = getattr(result, 'addSkip', None)
    if addSkip is not None:
        addSkip(test_case, reason)
    else:
        warnings.warn("TestResult has no addSkip method, skips not reported",
                      RuntimeWarning, 2)
        result.addSuccess(test_case)


def _addError(result, test, exc_info):
    if result is not None and exc_info is not None:
        if issubclass(exc_info[0], test.failureException):
            result.addFailure(test, exc_info)
        else:
            result.addError(test, exc_info)
