import io
import sys
import unittest


class Example(unittest.TestCase):
    def test_fail(self):
        """Hidden when descriptions are disabled."""
        self.fail("deliberate assertion")

    def test_skip(self):
        self.skipTest("fixture reason")


class SpecificError(ValueError):
    pass


class CustomFailure(unittest.TestCase):
    failureException = ValueError

    def test_failure(self):
        raise SpecificError("selected exception family")

    def test_error(self):
        raise AssertionError("unrelated exception")


def main():
    failed = Example("test_fail")
    skipped = Example("test_skip")
    suite = unittest.TestSuite([failed, skipped])
    stream = io.StringIO()
    result = unittest.TextTestRunner(stream, descriptions=False, verbosity=2).run(suite)
    assert suite.countTestCases() == result.testsRun == 2
    assert not result.wasSuccessful()
    assert len(result.failures) == 1 and len(result.errors) == 0
    assert result.failures[0][0] is failed
    assert "AssertionError: deliberate assertion" in result.failures[0][1]
    assert len(result.skipped) == 1 and result.skipped[0][0] is skipped
    assert result.skipped[0][1] == "fixture reason"
    text = stream.getvalue()
    assert "Hidden when descriptions are disabled." not in text
    assert "test_skip (__main__.Example.test_skip) ... skipped 'fixture reason'" in text
    assert "FAILED (failures=1, skipped=1)" in text
    selected = CustomFailure("test_failure")
    unrelated = CustomFailure("test_error")
    custom = unittest.TestSuite([selected, unrelated]).run(unittest.TestResult())
    assert not custom.wasSuccessful() and custom.testsRun == 2
    assert len(custom.failures) == 1 and custom.failures[0][0] is selected
    assert len(custom.errors) == 1 and custom.errors[0][0] is unrelated
    print("checked failures, skips, case identity and output")
    if not result.wasSuccessful():
        sys.exit(1)


main()
