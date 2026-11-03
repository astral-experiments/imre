import io
import sys
import traceback
import unittest


def current():
    test = unittest.TestCase()
    subtest = object()
    failure = (AssertionError, AssertionError("failure"), None)
    events = []

    class Result(unittest.TestResult):
        def _exc_info_to_string(self, err, owner):
            events.append((err, owner, self.shouldStop))
            return "from formatter"

    result = Result()
    result.failfast = True
    result.addSubTest(test, subtest, failure)
    assert events == [(failure, test, True)]
    assert result.failures == [(subtest, "from formatter")]

    def reject(err, owner):
        raise RuntimeError("formatter error")

    result._exc_info_to_string = reject
    with test.assertRaisesRegex(RuntimeError, "formatter error"):
        result.addError(test, None)
    assert result.errors == []

    # Capture the unfiltered exception before the result trims its live traceback.
    try:
        test.fail("user failure")
    except AssertionError as exc:
        head = exc.__traceback__
        framework = head.tb_next
        assert framework.tb_frame.f_globals["__unittest"]
        raw = "".join(traceback.format_exception(type(exc), exc, head))
        assert "raise self.failureException(msg)" in raw

        result = unittest.TestResult()
        result.addFailure(test, (type(exc), exc, head))
        report = result.failures[0][1]
        assert "raise self.failureException(msg)" not in report
        assert 'test.fail("user failure")' in report
        assert head is exc.__traceback__ and head.tb_next is None

    check_result_formatter(test)
    check_real_traceback(test)
    check_runner_locals()


def check_result_formatter(test):
    from unittest.result import TestResult
    assert TestResult is unittest.TestResult
    assert unittest.result is sys.modules["unittest.result"]

    class Provider:
        def TracebackException(self, kind, value, tb, *, capture_locals, compact):
            assert kind is AssertionError and value is failure and tb is head
            assert tb.tb_next is None and capture_locals is True and compact is True
            return self

        def format(self, *, colorize):
            assert colorize is False
            yield "first"
            if self.broken:
                raise ValueError("broken iterator")
            sys.stderr.write("during format")
            yield " second"

    provider = Provider()
    provider.broken = False
    try:
        test.fail("trim before calling provider")
    except AssertionError as exc:
        failure, head = exc, exc.__traceback__
    assert head.tb_next is not None
    assert unittest.result.traceback is traceback
    test.addCleanup(setattr, unittest.result, "traceback", traceback)
    old_out, old_err = sys.stdout, sys.stderr
    try:
        sys.stdout, sys.stderr = io.StringIO(), io.StringIO()
        result = unittest.TestResult()
        result.tb_locals = result.buffer = True
        result.startTest(test)
        unittest.result.traceback = provider
        err = (type(failure), failure, head)
        try:
            sys.stdout.write("before")
            result.addFailure(test, err)
            assert result.failures == [
                (test, "first second\nStdout:\nbefore\n\nStderr:\nduring format\n")
            ]
            provider.broken = True
            with test.assertRaisesRegex(ValueError, "broken iterator"):
                result.addError(test, err)
            assert result.errors == []
        finally:
            result.stopTest(test)
        assert sys.stdout.getvalue() == "\nStdout:\nbefore\n"
        assert sys.stderr.getvalue() == "\nStderr:\nduring format\n"
    finally:
        sys.stdout, sys.stderr = old_out, old_err
        test.doCleanups()
    assert unittest.result.traceback is traceback


def check_real_traceback(test):
    def capture(marker, cause):
        try:
            raise KeyError("hidden context")
        except KeyError:
            try:
                raise RuntimeError("outer") from cause
            except RuntimeError:
                result = unittest.TestResult()
                result.tb_locals = True
                result.addError(test, sys.exc_info())
                return result.errors[0][1]

    marker = [17]
    with_cause = capture(marker, ValueError("visible cause"))
    suppressed = capture(marker, None)
    marker.append(23)
    for message in (with_cause, suppressed):
        lines = message.splitlines()
        assert "    marker = [17]" in lines
        assert lines[-1] == "RuntimeError: outer"
        assert "KeyError: 'hidden context'" not in lines
        assert "During handling of the above exception" not in message
    direct = "The above exception was the direct cause of the following exception:"
    lines = with_cause.splitlines()
    assert lines.index("ValueError: visible cause") < lines.index(direct)
    assert direct not in suppressed


def check_runner_locals():
    class CutTail:
        def __repr__(self):
            self.head.tb_next = None
            return "cut tail"

    def inner():
        raise ValueError("application error")

    class Case(unittest.TestCase):
        def runTest(self):
            cut = CutTail()
            try:
                inner()
            except ValueError as exc:
                cut.head = exc.__traceback__
                raise

    stream = io.StringIO()
    result = unittest.TextTestRunner(stream=stream, tb_locals=True).run(Case())
    [(case, report)] = result.errors
    assert result.tb_locals and "    cut = cut tail" in report.splitlines()
    assert report in stream.getvalue()
    assert not any(line.endswith(", in inner") for line in report.splitlines())
