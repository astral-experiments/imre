# Adapted from CPython 3.14.5 Lib/unittest/case.py (PSF-2.0).
from functools import update_wrapper as _update_wrapper
from types import FunctionType as _FunctionType


def _identity(test_item):
    return test_item


def skip(reason):
    def decorator(test_item):
        if not isinstance(test_item, type):
            def skip_wrapper(*args, **kwargs):
                raise SkipTest(reason)
            test_item = _update_wrapper(skip_wrapper, test_item)
        test_item.__unittest_skip__ = True
        test_item.__unittest_skip_why__ = reason
        return test_item

    if isinstance(reason, _FunctionType):
        test_item = reason
        reason = ""
        return decorator(test_item)
    return decorator


def skipIf(condition, reason):
    if condition:
        return skip(reason)
    return _identity


def skipUnless(condition, reason):
    if not condition:
        return skip(reason)
    return _identity


def _boxedSkipTest(self, reason):
    raise SkipTest(reason)


def _boxedSkipCase(self, case, name, reason):
    self.startTest(case)
    try:
        self.addSkip(case, reason)
    finally:
        self.stopTest(case)


TestCase.skipTest = _boxedSkipTest
TestResult._skipCase = _boxedSkipCase
