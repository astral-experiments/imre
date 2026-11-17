"""Generated finite unittest launcher. Do not edit the generated copy."""
import json
import sys
import unittest


def emit(event):
    print("IMRE_UNITTEST " + json.dumps(event, ensure_ascii=True), file=sys.stderr, flush=True)


class OriginalResult(unittest.TestResult):
    def __init__(self):
        super().__init__()
        self.current = None
        self.outcome = None
        self.details = []

    def startTest(self, test):
        super().startTest(test)
        self.current = test
        self.outcome = "pass"
        self.details = []
        emit({"event": "start", "id": test.id()})

    def record(self, test, outcome, detail=None):
        if test is self.current:
            # An error takes precedence over a failure across multiple subtests.
            if self.outcome != "error" or outcome == "error":
                self.outcome = outcome
            if detail is not None:
                self.details.append(detail)
        else:
            emit({"event": "fixture", "id": test.id(), "outcome": outcome, "detail": detail})

    def addFailure(self, test, err):
        super().addFailure(test, err)
        self.record(test, "fail", str(err[1]))

    def addError(self, test, err):
        super().addError(test, err)
        self.record(test, "error", str(err[1]))

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        if test is self.current:
            self.record(test, "skip", reason)
        elif self.current is not None:
            emit({"event": "subtest_skip", "id": test.id(), "reason": reason})
        else:
            self.record(test, "skip", reason)

    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        if err is not None:
            outcome = "fail" if issubclass(err[0], test.failureException) else "error"
            self.record(test, outcome, subtest.id() + ": " + str(err[1]))

    def addExpectedFailure(self, test, err):
        super().addExpectedFailure(test, err)
        self.record(test, "expected_failure", str(err[1]))

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        self.record(test, "unexpected_success")

    def stopTest(self, test):
        emit({"event": "result", "id": test.id(), "outcome": self.outcome, "details": self.details})
        super().stopTest(test)
        self.current = None


WANTED = ('test.test_json.TestPyTest.test_pyjson', 'test.test_json.TestCTest.test_cjson', 'test.test_json.TestPyTest.test_pyjson', 'test.test_json.TestCTest.test_cjson')


def select_originals(source, target, wanted):
    for original in source:
        if isinstance(original, unittest.TestSuite):
            select_originals(original, target, wanted)
        elif original.id() in wanted:
            target.addTest(original)


def collect():
    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    import doctest
    import json as _json
    import json.encoder as _encoder
    import json.decoder as _decoder
    import test.test_json as _package
    for _occurrence in range(2):
        suite.addTest(doctest.DocTestSuite(_json))
        suite.addTest(doctest.DocTestSuite(_encoder))
        suite.addTest(doctest.DocTestSuite(_decoder))
        suite.addTest(_package.TestPyTest("test_pyjson"))
        suite.addTest(_package.TestCTest("test_cjson"))
    import test.test_json.test_decode as _module_0
    suite.addTests(loader.loadTestsFromModule(_module_0))
    import test.test_json.test_default as _module_1
    suite.addTests(loader.loadTestsFromModule(_module_1))
    import test.test_json.test_dump as _module_2
    suite.addTests(loader.loadTestsFromModule(_module_2))
    import test.test_json.test_encode_basestring_ascii as _module_3
    suite.addTests(loader.loadTestsFromModule(_module_3))
    import test.test_json.test_enum as _module_4
    suite.addTests(loader.loadTestsFromModule(_module_4))
    import test.test_json.test_fail as _module_5
    suite.addTests(loader.loadTestsFromModule(_module_5))
    import test.test_json.test_float as _module_6
    suite.addTests(loader.loadTestsFromModule(_module_6))
    import test.test_json.test_indent as _module_7
    suite.addTests(loader.loadTestsFromModule(_module_7))
    import test.test_json.test_pass1 as _module_8
    suite.addTests(loader.loadTestsFromModule(_module_8))
    import test.test_json.test_pass2 as _module_9
    suite.addTests(loader.loadTestsFromModule(_module_9))
    import test.test_json.test_pass3 as _module_10
    suite.addTests(loader.loadTestsFromModule(_module_10))
    import test.test_json.test_recursion as _module_11
    suite.addTests(loader.loadTestsFromModule(_module_11))
    import test.test_json.test_scanstring as _module_12
    suite.addTests(loader.loadTestsFromModule(_module_12))
    import test.test_json.test_separators as _module_13
    suite.addTests(loader.loadTestsFromModule(_module_13))
    import test.test_json.test_speedups as _module_14
    suite.addTests(loader.loadTestsFromModule(_module_14))
    import test.test_json.test_tool as _module_15
    suite.addTests(loader.loadTestsFromModule(_module_15))
    import test.test_json.test_unicode as _module_16
    suite.addTests(loader.loadTestsFromModule(_module_16))
    selected = unittest.TestSuite()
    select_originals(suite, selected, WANTED)
    return selected


def inventory(suite):
    for test in suite:
        if isinstance(test, unittest.TestSuite):
            inventory(test)
        else:
            emit({"event": "inventory", "id": test.id()})


def main():
    suite = collect()
    emit({"event": "collection", "count": suite.countTestCases()})
    inventory(suite)
    result = OriginalResult()
    suite.run(result)
    emit({"event": "summary", "run": result.testsRun, "successful": result.wasSuccessful()})
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
