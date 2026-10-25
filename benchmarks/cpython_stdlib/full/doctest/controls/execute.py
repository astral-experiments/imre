import doctest
import examples
import mismatch

suite = doctest.DocTestSuite(examples)
print(suite.countTestCases())
suite.debug()
for case in suite:
    case.runTest()
    print(case.id())

wrong = doctest.DocTestSuite(mismatch)
for case in wrong:
    try:
        case.debug()
    except doctest.DocTestFailure:
        print("debug stopped", len(mismatch.visits))
    try:
        case.runTest()
    except AssertionError as exc:
        print("checked output", "Failed example:" in str(exc), len(mismatch.visits))
