import doctest
import unittest
import sample_doctest_skip

suite = doctest.DocTestSuite(sample_doctest_skip)
print(suite.countTestCases())
for case in suite:
    print(case.id())
    print(len(case._dt_test.examples))
    try:
        case.runTest()
    except unittest.SkipTest as exc:
        print("skipped: " + str(exc))
    except AssertionError as exc:
        print("mismatch" if "Failed example:" in str(exc) else "wrong assertion")
    else:
        print("passed")
    try:
        case.debug()
    except doctest.DocTestFailure:
        print("debug mismatch")
    else:
        print("debug completed")
