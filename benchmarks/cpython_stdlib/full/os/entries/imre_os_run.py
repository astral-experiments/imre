import test.test_os as suite_0
import test.test_posix as suite_1
import test.test_genericpath as suite_2
import unittest

tests = unittest.TestSuite([unittest.defaultTestLoader.loadTestsFromModule(suite_0), unittest.defaultTestLoader.loadTestsFromModule(suite_1), unittest.defaultTestLoader.loadTestsFromModule(suite_2)])
result = unittest.TextTestRunner(verbosity=2).run(tests)
print('IMRE_SUITE_RESULT', result.testsRun, len(result.failures), len(result.errors), len(result.skipped))
