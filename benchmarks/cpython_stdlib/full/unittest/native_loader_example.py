import unittest
import bar.test_bar as subject

suite = unittest.TestSuite([unittest.defaultTestLoader.loadTestsFromModule(subject)])
print(suite.countTestCases())
result = unittest.TextTestRunner(verbosity=2).run(suite)
print(result.testsRun)
print(result.wasSuccessful())
