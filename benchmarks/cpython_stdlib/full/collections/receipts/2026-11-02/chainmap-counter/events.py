"""Report the original unittest registrations and callbacks for the ledger."""
import sys
import unittest
from test import support

support.use_resources = {"cpu": None}
registered = {}
started = {}


def register(suite):
    for test in suite:
        if isinstance(test, unittest.TestSuite):
            register(test)
        else:
            name = test.id()
            registered[name] = registered.get(name, 0) + 1
            print("REGISTER", name, registered[name], sep="\t", flush=True)


def emit(kind, test, detail=""):
    name = test.id()
    print(kind, name, started.get(name, 0), repr(detail), sep="\t", flush=True)


class Events(unittest.TextTestResult):
    def startTest(self, test):
        name = test.id()
        started[name] = started.get(name, 0) + 1
        emit("START", test)
        super().startTest(test)

    def stopTest(self, test):
        emit("STOP", test)
        super().stopTest(test)

    def addSuccess(self, test):
        emit("PASS", test)
        super().addSuccess(test)

    def addSkip(self, test, reason):
        emit("SKIP", test, reason)
        super().addSkip(test, reason)

    def addFailure(self, test, err):
        emit("FAIL", test, str(err[1]))
        super().addFailure(test, err)

    def addError(self, test, err):
        emit("ERROR", test, str(err[1]))
        super().addError(test, err)

    def addExpectedFailure(self, test, err):
        emit("XFAIL", test, str(err[1]))
        super().addExpectedFailure(test, err)

    def addUnexpectedSuccess(self, test):
        emit("XPASS", test)
        super().addUnexpectedSuccess(test)

    def addSubTest(self, test, subtest, err):
        emit("SUBTEST", test, (subtest.id(), None if err is None else str(err[1])))
        super().addSubTest(test, subtest, err)


def run(suite, expected):
    register(suite)
    count = suite.countTestCases()
    print("REGISTERED_COUNT", count, sep="\t", flush=True)
    result = unittest.TextTestRunner(verbosity=2, descriptions=False, resultclass=Events).run(suite)
    print("RUNNER_COUNT", result.testsRun, sep="\t", flush=True)
    valid = result.wasSuccessful() and count == expected and result.testsRun == count and registered == started
    sys.exit(0 if valid else 1)


def load(module):
    print("MODULE", module.__name__, module.__file__, sep="\t", flush=True)
    loader = unittest.TestLoader()
    suite = loader.loadTestsFromModule(module)
    for error in loader.errors:
        print("LOADER_ERROR", repr(error), sep="\t", flush=True)
    return suite
