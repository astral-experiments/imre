def current():
    import unittest

    def test_method(value):
        raise AssertionError("skipped method ran")

    assert unittest.skipIf(False, "disabled")(test_method) is test_method
    assert unittest.skipUnless(True, "enabled")(test_method) is test_method

    for wrapper in (unittest.skip("reason")(test_method), unittest.skip(test_method)):
        assert wrapper.__unittest_skip__
        assert wrapper.__wrapped__ is test_method
        assert wrapper.__name__ == test_method.__name__
        try:
            wrapper(1)
        except unittest.SkipTest as exc:
            assert exc.args == (wrapper.__unittest_skip_why__,)
            print(exc.args)
        else:
            raise AssertionError("skipped method returned")

    class Example(unittest.TestCase):
        pass

    assert unittest.skipUnless(False, "missing feature")(Example) is Example
    assert Example.__unittest_skip__
    print(Example.__unittest_skip_why__)

    class Child(Example):
        def test_skipped(self):
            raise AssertionError("inherited skip ran the test")

    reason = object()
    unittest.skip(reason)(Example)
    case = Child("test_skipped")
    result = unittest.TestResult()
    unittest.TestSuite((case,)).run(result)
    assert result.testsRun == 1
    assert result.skipped[0][0] is case
    assert result.skipped[0][1] is reason

    try:
        case.skipTest(reason)
    except unittest.SkipTest as exc:
        assert exc.args[0] is reason
    else:
        raise AssertionError("skipTest returned")

    @unittest.expectedFailure
    class Expected(unittest.TestCase):
        def test_error(self):
            raise ValueError("from the body")

        def test_pass(self):
            pass

        def test_cleanup(self):
            self.addCleanup(self.fail, "from cleanup")
            self.fail("from the body")

    failed = Expected("test_error")
    passed = Expected("test_pass")
    cleanup = Expected("test_cleanup")
    result = unittest.TestResult()
    result.failfast = True
    failed.run(result)
    assert result.wasSuccessful() and not result.shouldStop
    assert result.expectedFailures[0][0] is failed
    assert "ValueError: from the body" in result.expectedFailures[0][1]
    cleanup.run(result)
    assert result.failures[0][0] is cleanup and "from cleanup" in result.failures[0][1]
    assert len(result.expectedFailures) == 1
    passed.run(result)
    assert result.unexpectedSuccesses == [passed] and result.shouldStop

    for name, test in (("addExpectedFailure", failed), ("addUnexpectedSuccess", passed)):
        observer = unittest.TestResult()
        setattr(observer, name, None)
        try:
            test.run(observer)
        except TypeError:
            pass
        else:
            raise AssertionError("non-callable result callback was treated as missing")
