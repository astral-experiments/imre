import sys
import unittest
from test import test_typing

class Events(unittest.TextTestResult):
    def startTest(self, case):
        print('START', case.id(), flush=True)
        super().startTest(case)

    def addSuccess(self, case):
        print('PASS', case.id(), flush=True)
        super().addSuccess(case)

    def addFailure(self, case, error):
        print('FAIL', case.id(), str(error[1]), flush=True)
        super().addFailure(case, error)

    def addError(self, case, error):
        print('ERROR', case.id(), str(error[1]), flush=True)
        super().addError(case, error)

    def addSkip(self, case, reason):
        print('SKIP', case.id(), reason, flush=True)
        super().addSkip(case, reason)

print('IMPORT_OK test.test_typing', flush=True)
suite = unittest.defaultTestLoader.loadTestsFromModule(test_typing)
print('REGISTERED_COUNT', suite.countTestCases(), flush=True)
result = unittest.TextTestRunner(verbosity=2, descriptions=False, resultclass=Events).run(suite)
print('RUNNER_COUNT', result.testsRun, flush=True)
sys.exit(0 if result.wasSuccessful() else 1)
