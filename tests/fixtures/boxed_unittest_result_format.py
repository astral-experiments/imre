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
