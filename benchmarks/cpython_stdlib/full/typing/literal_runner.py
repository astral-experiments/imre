import sys
import unittest

class ObservedContext:
    def __init__(self, original, case, ordinal, params):
        self.original = original
        self.case = case
        self.ordinal = ordinal
        self.params = params

    def __enter__(self):
        return self.original.__enter__()

    def __exit__(self, typ, value, traceback):
        try:
            suppress = self.original.__exit__(typ, value, traceback)
        except BaseException:
            print('SUBTEST_EVENT', self.case.id(), self.ordinal, 'ERROR', repr(self.params), flush=True)
            raise
        outcome = 'PASS'
        if value is not None:
            if isinstance(value, self.case.failureException):
                outcome = 'FAIL'
            elif isinstance(value, unittest.SkipTest):
                outcome = 'SKIP'
            else:
                outcome = 'ERROR'
        print('SUBTEST_EVENT', self.case.id(), self.ordinal, outcome, repr(self.params), flush=True)
        return suppress

class ObservedSubtest:
    def __init__(self, case):
        self.case = case
        self.original = case.subTest
        self.ordinal = 0

    def __call__(self, *args, **params):
        context = self.original(*args, **params)
        self.ordinal += 1
        return ObservedContext(context, self.case, self.ordinal, params)

print('IMPORT_BEGIN test.test_typing', flush=True)
from test import test_typing
print('IMPORT_OK test.test_typing', flush=True)
suite = unittest.defaultTestLoader.loadTestsFromTestCase(test_typing.LiteralTests)
for case in suite:
    owner = type(case).__module__ + '.' + type(case).__qualname__
    print('REGISTERED_ID', case.id(), owner, flush=True)
    if case.id().endswith('.test_dir'):
        case.subTest = ObservedSubtest(case)
print('REGISTERED_COUNT', suite.countTestCases(), flush=True)
result = unittest.TextTestRunner(verbosity=2, descriptions=False).run(suite)
print('RUNNER_COUNT', result.testsRun, flush=True)
sys.exit(0 if result.wasSuccessful() else 1)
