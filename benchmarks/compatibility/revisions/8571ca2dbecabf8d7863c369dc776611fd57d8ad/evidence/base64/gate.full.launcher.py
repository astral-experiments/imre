"""Generated finite unittest launcher. Do not edit the generated copy."""
import gate_control_base64
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


def collect():
    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    import test.test_base64 as _module_0
    suite.addTests(loader.loadTestsFromModule(_module_0))
    return suite


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
