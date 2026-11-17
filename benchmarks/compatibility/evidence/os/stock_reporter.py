"""Run an intact upstream suite, printing one canonical outcome per original.

Descriptions are disabled because unittest otherwise splits a result over two
lines. Failing subtests remain in TestResult's errors/failures and are reported
at their parent original's stopTest; no test or fixture is changed.
"""
import sys
import unittest

class OriginalResult(unittest.TextTestResult):
    def getDescription(self, test):
        original_id = test.id()
        label = getattr(test, '_testMethodName', original_id)
        return f'{label} ({original_id})'

    def startTest(self, test):
        self._subtest_outcome = None
        super().startTest(test)

    def addSubTest(self, test, subtest, err):
        unittest.TestResult.addSubTest(self, test, subtest, err)
        if err is not None:
            outcome = 'FAIL' if issubclass(err[0], subtest.failureException) else 'ERROR'
            if self._subtest_outcome != 'ERROR':
                self._subtest_outcome = outcome

    def stopTest(self, test):
        if self._subtest_outcome:
            self._write_status(test, self._subtest_outcome)
        super().stopTest(test)

if __name__ == '__main__':
    suite = unittest.defaultTestLoader.loadTestsFromName(sys.argv[1])
    result = unittest.TextTestRunner(verbosity=2, descriptions=False,
                                     resultclass=OriginalResult).run(suite)
    sys.exit(not result.wasSuccessful())
