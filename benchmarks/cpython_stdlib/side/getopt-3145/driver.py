import sys
import unittest
import test.test_getopt as original

def visit(suite):
    for test in suite:
        if isinstance(test, unittest.TestSuite):
            visit(test)
        else:
            print("ID " + test.id())

if sys.argv[1] == "module":
    suite = unittest.defaultTestLoader.loadTestsFromModule(original)
else:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(original.GetoptTests)
visit(suite)
print("REGISTERED " + str(suite.countTestCases()))
outcome = unittest.TextTestRunner(verbosity=2).run(suite)
if not outcome.wasSuccessful():
    raise SystemExit(1)
