import collections
import hashlib
import json
from pathlib import Path
import sys
import unittest

from test import test_typing
import typing

out = Path(sys.argv[1])
suite = unittest.defaultTestLoader.loadTestsFromModule(test_typing)
print("python", sys.version)
print("suite", test_typing.__file__, hashlib.sha256(Path(test_typing.__file__).read_bytes()).hexdigest())
print("typing", typing.__file__)
print("discovered", suite.countTestCases())

class Ledger(unittest.TestResult):
    def __init__(self, stream):
        super().__init__()
        self.stream = stream
        self.counts = collections.Counter()

    def record(self, case, status, detail=None):
        row = {"id": case.id(), "status": status}
        if detail is not None:
            row["detail"] = detail
        print(json.dumps(row, sort_keys=True), file=self.stream, flush=True)
        self.counts[status] += 1

    def addSuccess(self, case):
        super().addSuccess(case)
        self.record(case, "pass")

    def addSkip(self, case, reason):
        super().addSkip(case, reason)
        self.record(case, "skip", reason)

    def addError(self, case, error):
        super().addError(case, error)
        self.record(case, "error", self.errors[-1][1])

    def addFailure(self, case, error):
        super().addFailure(case, error)
        self.record(case, "fail", self.failures[-1][1])

    def addSubTest(self, case, subtest, error):
        super().addSubTest(case, subtest, error)
        if error:
            self.record(case, "subtest-fail", (subtest.id(), self._exc_info_to_string(error, case)))

    def addExpectedFailure(self, case, error):
        super().addExpectedFailure(case, error)
        self.record(case, "expected-failure", self.expectedFailures[-1][1])

    def addUnexpectedSuccess(self, case):
        super().addUnexpectedSuccess(case)
        self.record(case, "unexpected-success")

with out.open("w") as stream:
    result = Ledger(stream)
    suite.run(result)
print("ran", result.testsRun, "outcomes", dict(result.counts))
print("successful", result.wasSuccessful())
raise SystemExit(not result.wasSuccessful())
