import doctest
import examples
import json
import mismatch

# Executing one suite must not compile an unrelated module's example bodies.
unrelated = (json, json.encoder)
selected = examples
suite = doctest.DocTestSuite(selected)
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
        print("debug stopped")
        print(len(mismatch.visits))
    try:
        case.runTest()
    except AssertionError as exc:
        print("checked output")
        print("Failed example:" in str(exc))
        print(len(mismatch.visits))
