import sys
import unittest
from test import test_re


class ReportingResult(unittest.TextTestResult):
    def addSubTest(self, test, subtest, err):
        if err is None:
            print("SUB_OK", test.id(), flush=True)
        unittest.TextTestResult.addSubTest(self, test, subtest, err)


wanted = sys.argv[1:]
assert wanted and len(set(wanted)) == len(wanted)
selected = unittest.TestSuite()
for name in wanted:
    parts = name.split(".")
    assert len(parts) == 4 and parts[:2] == ["test", "test_re"]
    assert parts[2] in ("ReTests", "DebugTests", "ImplementationTest", "PatternReprTests")
    assert parts[3].startswith("test_")
    cls = getattr(test_re, parts[2])
    case = cls(parts[3])
    assert case.id() == name
    selected.addTest(case)
    print("SELECT", name)
print("ORIGINAL_MODULE", test_re.__file__, flush=True)
print("SELECTED", selected.countTestCases(), flush=True)
result = unittest.TextTestRunner(verbosity=2, resultclass=ReportingResult).run(selected)
for label, items in (("FAIL", result.failures), ("ERROR", result.errors),
                     ("SKIP", result.skipped), ("XFAIL", result.expectedFailures)):
    for test, detail in items:
        print(label, repr(test.id()), repr(detail), sep="\t")
for test in result.unexpectedSuccesses:
    print("XPASS", repr(test.id()), sep="\t")
print("RAN", result.testsRun, flush=True)
assert result.testsRun == len(wanted)
raise SystemExit(not result.wasSuccessful())
