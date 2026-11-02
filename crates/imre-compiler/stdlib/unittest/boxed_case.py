import contextlib
import time
import warnings
from collections import ChainMap
from unittest import SkipTest, TestCase, _enterCleanupContext

DIFF_OMITTED = ('\nDiff is %s characters long. '
                 'Set self.maxDiff to None to see it.')

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
def expectedFailure(test_item):
    test_item.__unittest_expecting_failure__ = True
    return test_item


class _BaseTestCaseContext:

    def __init__(self, test_case):
        self.test_case = test_case

    def _raiseFailure(self, standardMsg):
        msg = self.test_case._formatMessage(self.msg, standardMsg)
        raise self.test_case.failureException(msg)


def assertLogs(self, logger=None, level=None):
    """Fail unless a log message of level *level* or higher is emitted
    on *logger_name* or its children.  If omitted, *level* defaults to
    INFO and *logger* defaults to the root logger.

    This method must be used as a context manager, and will yield
    a recording object with two attributes: `output` and `records`.
    At the end of the context manager, the `output` attribute will
    be a list of the matching formatted log messages and the
    `records` attribute will be a list of the corresponding LogRecord
    objects.

    Example::

        with self.assertLogs('foo', level='INFO') as cm:
            logging.getLogger('foo').info('first message')
            logging.getLogger('foo.bar').error('second message')
        self.assertEqual(cm.output, ['INFO:foo:first message',
                                     'ERROR:foo.bar:second message'])
    """
    # Lazy import to avoid importing logging if it is not needed.
    from ._log import _AssertLogsContext
    return _AssertLogsContext(self, logger, level, no_logs=False)


def assertNoLogs(self, logger=None, level=None):
    """ Fail unless no log messages of level *level* or higher are emitted
    on *logger_name* or its children.

    This method must be used as a context manager.
    """
    from ._log import _AssertLogsContext
    return _AssertLogsContext(self, logger, level, no_logs=True)


assertLogs.__qualname__ = "TestCase.assertLogs"
assertNoLogs.__qualname__ = "TestCase.assertNoLogs"
TestCase.assertLogs = assertLogs
TestCase.assertNoLogs = assertNoLogs
del assertLogs, assertNoLogs


class _UnexpectedSuccess(Exception):
    pass


def _addExpectedFailure(self, result, exc_info):
    try:
        callback = result.addExpectedFailure
    except AttributeError:
        warnings.warn("TestResult has no addExpectedFailure method, reporting as passes", RuntimeWarning)
        result.addSuccess(self)
    else:
        callback(self, exc_info)


def _addUnexpectedSuccess(self, result):
    try:
        callback = result.addUnexpectedSuccess
    except AttributeError:
        import sys
        warnings.warn("TestResult has no addUnexpectedSuccess method, reporting as failure", RuntimeWarning)
        try:
            raise _UnexpectedSuccess from None
        except _UnexpectedSuccess:
            result.addFailure(self, sys.exc_info())
    else:
        callback(self)


class _ShouldStop(Exception):
    """
    The test should stop.
    """


_subtest_msg_sentinel = object()


class _OrderedChainMap(ChainMap):
    def __iter__(self):
        seen = set()
        for mapping in self.maps:
            for key in mapping:
                if key not in seen:
                    seen.add(key)
                    yield key


class _SubTest(TestCase):
    def __init__(self, test_case, message, params):
        super().__init__()
        self.test_case = test_case
        self.failureException = test_case.failureException
        self._message = message
        self.params = params

    def _subDescription(self):
        parts = []
        if self._message is not _subtest_msg_sentinel:
            parts.append("[" + str(self._message) + "]")
        if self.params:
            items = [key + "=" + repr(self.params[key]) for key in self.params]
            parts.append("(" + ", ".join(items) + ")")
        return " ".join(parts) or "(<subtest>)"

    def id(self):
        return self.test_case.id() + " " + self._subDescription()

    def __str__(self):
        return str(self.test_case) + " " + self._subDescription()

    def shortDescription(self):
        return self.test_case.shortDescription()

    def runTest(self):
        raise NotImplementedError("subtests cannot be run directly")


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


# From CPython 3.14.5 Lib/unittest/case.py. See ../docstrings/LICENSE.
def _addDuration(self, result, elapsed):
    try:
        addDuration = result.addDuration
    except AttributeError:
        warnings.warn("TestResult has no addDuration method",
                      RuntimeWarning)
    else:
        addDuration(self, elapsed)


TestCase._addDuration = _addDuration


def _addSkip(result, test_case, reason):
    addSkip = getattr(result, 'addSkip', None)
    if addSkip is not None:
        addSkip(test_case, reason)
        from unittest import TextTestResult
        if isinstance(result, TextTestResult):
            status = "skipped " + repr(reason)
            if isinstance(test_case, _SubTest):
                result._markSubTest(test_case._subDescription(), "s", status)
            else:
                result._mark("s", status)
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
