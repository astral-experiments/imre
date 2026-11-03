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
