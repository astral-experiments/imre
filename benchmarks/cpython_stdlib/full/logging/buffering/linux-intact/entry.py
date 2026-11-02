import os
import sys
import unittest


def event(tag, *items):
    print('IMRE_INTACT', os.getpid(), tag, *items, flush=True)


class Observed(unittest.TestResult):
    def startTestRun(self):
        event('RUN_START')
        return super().startTestRun()

    def stopTestRun(self):
        event('RUN_STOP')
        return super().stopTestRun()

    def startTest(self, test):
        event('START', test.id())
        return super().startTest(test)

    def stopTest(self, test):
        event('STOP', test.id())
        return super().stopTest(test)

    def addSuccess(self, test):
        event('PASS', test.id())
        return super().addSuccess(test)

    def addFailure(self, test, err):
        event('FAIL', test.id(), type(err[1]).__name__, repr(str(err[1])))
        return super().addFailure(test, err)

    def addError(self, test, err):
        event('ERROR', test.id(), type(err[1]).__name__, repr(str(err[1])))
        return super().addError(test, err)

    def addSkip(self, test, reason):
        event('SKIP', test.id(), repr(str(reason)))
        return super().addSkip(test, reason)

    def addExpectedFailure(self, test, err):
        event('XFAIL', test.id(), type(err[1]).__name__, repr(str(err[1])))
        return super().addExpectedFailure(test, err)

    def addUnexpectedSuccess(self, test):
        event('XPASS', test.id())
        return super().addUnexpectedSuccess(test)

    def addSubTest(self, test, subtest, err):
        if err is None:
            event('SUBPASS', test.id(), subtest.id())
        else:
            event('SUBERROR', test.id(), subtest.id(), type(err[1]).__name__, repr(str(err[1])))
        return super().addSubTest(test, subtest, err)


def main():
    from test import test_logging
    suite = unittest.TestLoader().loadTestsFromModule(test_logging)
    event('DISCOVERED', suite.countTestCases())
    result = Observed()
    result.startTestRun()
    suite.run(result)
    result.stopTestRun()
    event('TOTAL', result.testsRun, len(result.failures), len(result.errors), len(result.skipped))
    return 0 if result.wasSuccessful() else 1


if __name__ == '__main__':
    sys.exit(main())
