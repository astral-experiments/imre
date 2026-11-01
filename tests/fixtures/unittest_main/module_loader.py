import sys
import unittest
import hook_cases


loader = unittest.TestLoader()
loader.testNamePatterns = ["*.test_bad", "*.test_skip"]
suite = loader.loadTestsFromModule(hook_cases, pattern="test_*.py")
result = unittest.TextTestRunner(verbosity=2).run(suite)
print(result.testsRun)
print(len(result.failures))
print(len(result.errors))
for test, reason in result.skipped:
    print(test.id())
    print(reason)
if not result.wasSuccessful():
    sys.exit(1)
