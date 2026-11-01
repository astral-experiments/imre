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
