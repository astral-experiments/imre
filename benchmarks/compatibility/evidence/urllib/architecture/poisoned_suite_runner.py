"""Run an intact upstream unittest module and emit one auditable line per original."""
import urllib_poison
import sys
import unittest


class OriginalResult(unittest.TestResult):
    def __init__(self):
        super().__init__()
        self.current = None
        self.status = None
        self.details = []
        self.originals = 0
        self.subtest_skips = 0

    def startTest(self, test):
        super().startTest(test)
        self.current = test
        self.status = 'ok'

    def addSuccess(self, test):
        super().addSuccess(test)

    def addFailure(self, test, err):
        super().addFailure(test, err)
        self.status = 'FAIL'
        self.details.append((test.id(), type(err[1]).__name__, str(err[1])))

    def addError(self, test, err):
        super().addError(test, err)
        self.status = 'ERROR'
        self.details.append((test.id(), type(err[1]).__name__, str(err[1])))

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        if test is self.current:
            self.status = 'skipped ' + repr(reason)
        else:
            self.subtest_skips += 1

    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        if err is not None:
            self.status = 'FAIL' if issubclass(err[0], test.failureException) else 'ERROR'
            self.details.append((subtest.id(), type(err[1]).__name__, str(err[1])))

    def addExpectedFailure(self, test, err):
        super().addExpectedFailure(test, err)
        self.status = 'expected failure'

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        self.status = 'unexpected success'

    def stopTest(self, test):
        print(f'{test._testMethodName} ({test.id()}) ... {self.status}', file=sys.stderr, flush=True)
        self.originals += 1
        super().stopTest(test)
        self.current = None


if sys.argv[1] == 'test.test_urlparse':
    import test.test_urlparse as module
elif sys.argv[1] == 'test.test_urllib':
    import test.test_urllib as module
else:
    raise SystemExit('unknown suite')
suite = unittest.defaultTestLoader.loadTestsFromModule(module)
expected = suite.countTestCases()
result = OriginalResult()
suite.run(result)
print(f'Ran {result.originals} tests in runner', file=sys.stderr)
print(f'collection={expected} subtest_skips={result.subtest_skips}', file=sys.stderr)
for test_id, kind, message in result.details:
    print(f'detail {test_id}: {kind}: {message!r}', file=sys.stderr)
if result.originals != expected:
    raise SystemExit(f'collection mismatch: {result.originals} != {expected}')
raise SystemExit(0 if result.wasSuccessful() else 1)
