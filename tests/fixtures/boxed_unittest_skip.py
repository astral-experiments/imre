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

        def test_skipped_then_expected(self):
            self.addCleanup(order.append, "cleanup")
            with self.subTest(part="skipped"):
                self.skipTest("skip")
            with self.subTest(part="expected"):
                self.fail("expected failure")
            order.append("continued")

    order = []
    mixed = Expected("test_skipped_then_expected").run()
    assert order == ["continued", "cleanup"]
    assert len(mixed.skipped) == 1 and not mixed.expectedFailures

    failed = Expected("test_error")
    passed = Expected("test_pass")
    cleanup = Expected("test_cleanup")
    result = unittest.TestResult()
    result.failfast = True
    failed.run(result)
    assert result.wasSuccessful() and not result.shouldStop
    method = unittest.TestResult.__dict__["wasSuccessful"]
    assert method.__name__ == "wasSuccessful" and method.__qualname__ == "TestResult.wasSuccessful"
    assert method.__doc__ == "Tells whether or not this result was a success."
    legacy = unittest.TestResult()
    del legacy.unexpectedSuccesses
    assert legacy.wasSuccessful()
    legacy.errors.append((failed, "error"))
    assert not legacy.wasSuccessful()
    assert result.expectedFailures[0][0] is failed
    assert "ValueError: from the body" in result.expectedFailures[0][1]
    cleanup.run(result)
    assert result.failures[0][0] is cleanup and "from cleanup" in result.failures[0][1]
    assert len(result.expectedFailures) == 1
    passed.run(result)
    assert result.unexpectedSuccesses == [passed] and result.shouldStop

    class StopRecorder(unittest.TestResult):
        def stop(self):
            stops.append((len(self.failures), len(self.errors), len(self.unexpectedSuccesses)))
            super().stop()

    for enabled in (False, True):
        observer = StopRecorder()
        if enabled:
            observer.failfast = True
        else:
            del observer.failfast
        stops = []
        error = (ValueError, ValueError("detail"), None)
        observer.addFailure(failed, error)
        observer.addError(failed, error)
        observer.addSubTest(failed, failed, None)
        observer.addSubTest(failed, failed, error)
        observer.addUnexpectedSuccess(passed)
        assert len(observer.failures) == 1 and len(observer.errors) == 2
        assert observer.unexpectedSuccesses == [passed] and observer.shouldStop == enabled
        assert stops == ([(0, 0, 0), (1, 0, 0), (1, 1, 0), (1, 2, 0)] if enabled else [])

    for name, test in (("addExpectedFailure", failed), ("addUnexpectedSuccess", passed)):
        observer = unittest.TestResult()
        setattr(observer, name, None)
        try:
            test.run(observer)
        except TypeError:
            pass
        else:
            raise AssertionError("non-callable result callback was treated as missing")

    events = []

    class Recording(unittest.TestResult):
        def addSubTest(self, parent, child, error):
            events.append((parent, child, error, parent._subtest))
            super().addSubTest(parent, child, error)

        def addSkip(self, child, reason):
            events.append((child.test_case, child, reason, child.test_case._subtest))
            super().addSkip(child, reason)

        def addSuccess(self, test):
            events.append(test)

    class Subtests(unittest.TestCase):
        def test_nested(self):
            values = []
            with self.subTest(group=1):
                parent_params = self._subtest.params
                with self.subTest(values=values):
                    values.append(2)
                parent_params["group"] = 3
                with self.subTest("broken"):
                    raise problem
                with self.subTest("skipped"):
                    self.skipTest("reason")
            with self.subTest(group=2):
                pass

    problem = AssertionError("body failed")
    parent = Subtests("test_nested")
    result = Recording()
    parent.run(result)
    assert [child.id() for _, child, _, _ in events] == [
        parent.id() + " (values=[2], group=3)",
        parent.id() + " [broken] (group=3)",
        parent.id() + " [skipped] (group=3)",
        parent.id() + " (group=2)",
    ]
    assert all(test is parent and child is active for test, child, _, active in events)
    assert events[0][2] is None and events[3][2] is None
    typ, value, tb = events[1][2]
    assert typ is AssertionError and value is problem and tb is not None
    assert result.failures[0][0] is events[1][1]
    assert result.skipped == [(events[2][1], "reason")]
    assert parent._subtest is None
