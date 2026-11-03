import io
import unittest


def current():
    events = []

    class Described:
        def shortDescription(self):
            events.append("description")
            return self.description

        def __str__(self):
            events.append("string")
            return "test name"

    test = Described()
    test.description = "doc line"
    result = unittest.TextTestResult(None, True, 0)
    assert result.getDescription(test) == "test name\ndoc line"
    result.descriptions = False
    assert result.getDescription(test) == "test name"
    assert events == ["description", "string"] * 2
    test.description = 42
    result.descriptions = True
    try:
        result.getDescription(test)
    except TypeError:
        pass
    else:
        raise AssertionError("shortDescription returned a non-string")

    inits = []

    class RecordingResult(unittest.TestResult):
        def __init__(self, stream, descriptions, verbosity):
            inits.append((stream, descriptions, verbosity))
            super().__init__(stream, descriptions, verbosity)

    class TextResult(unittest.TextTestResult, RecordingResult):
        pass

    for stream, descriptions, verbosity in ((io.StringIO(), object(), 2), (None, None, 1)):
        result = TextResult(stream, descriptions, verbosity, durations=3)
        assert inits == [(stream, descriptions, verbosity)]
        assert result.descriptions is descriptions
        assert result.testsRun == 0 and result.durations == 3
        inits.clear()

    class Case(unittest.TestCase):
        def test_failure(self):
            self.fail("failure")

        def test_success(self):
            pass

    class Result(unittest.TextTestResult):
        def getDescription(self, test):
            calls.append(test)
            return "label " + str(len(calls)) + "\ncustom line"

    for verbosity in (0, 1, 2, 3):
        calls = []
        stream = io.StringIO()
        failed = Case("test_failure")
        passed = Case("test_success")
        suite = unittest.TestSuite([passed, failed])
        unittest.TextTestRunner(stream=stream, descriptions=False, verbosity=verbosity,
                                resultclass=Result).run(suite)
        if verbosity > 1:
            assert calls == [passed, failed, failed]
            assert "label 1\ncustom line ... ok" in stream.getvalue()
            assert "FAIL: label 3\ncustom line" in stream.getvalue()
        else:
            assert calls == [failed]
            assert "FAIL: label 1\ncustom line" in stream.getvalue()

    stream = io.StringIO()
    output = unittest.TextTestRunner(stream=stream).stream
    result = Result(output, False, 2)
    calls = []
    result.addUnexpectedSuccess(passed)
    result.addExpectedFailure(failed, (ValueError, ValueError("bad"), None))
    assert calls == [] and stream.getvalue() == "unexpected success\nexpected failure\n"

    class Summary(unittest.TextTestResult):
        def printErrorList(self, flavour, errors):
            calls.append((flavour, errors))

    summary = Summary(output, False, 0)
    del summary.unexpectedSuccesses
    summary.printErrors()
    assert calls == [("ERROR", summary.errors), ("FAIL", summary.failures)]
