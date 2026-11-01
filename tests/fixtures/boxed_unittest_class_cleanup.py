import unittest

events = []


def note(value, *, suffix=""):
    events.append(value + suffix)


def cleanup_error():
    events.append("cleanup error")
    raise ValueError("cleanup failed")


class Base(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.addClassCleanup(note, cls.__name__, suffix=" earlier")
        cls.addClassCleanup(cls.late)

    @classmethod
    def late(cls):
        events.append(cls.__name__ + " late")
        cls.addClassCleanup(note, cls.__name__, suffix=" nested")

    def test_body(self):
        events.append(type(self).__name__ + " body")


class First(Base):
    pass


class Second(Base):
    pass


class SetupError(Base):
    @classmethod
    def setUpClass(cls):
        cls.addClassCleanup(note, "cleanup after setup failure")
        cls.addClassCleanup(cleanup_error)
        raise RuntimeError("setup failed")


class TeardownError(Base):
    @classmethod
    def tearDownClass(cls):
        raise RuntimeError("teardown failed")


def current():
    events.clear()
    # Register on both concrete subclasses before they enter a suite.
    First.addClassCleanup(note, "first registered")
    Second.addClassCleanup(note, "second registered")
    suite = unittest.TestSuite([First("test_body"), Second("test_body"),
                                SetupError("test_body"), TeardownError("test_body")])
    result = suite.run(unittest.TestResult())
    assert result.testsRun == 3 and not result.wasSuccessful()
    assert events == ["First body", "First late", "First nested", "First earlier", "first registered",
                      "Second body", "Second late", "Second nested", "Second earlier", "second registered",
                      "cleanup error", "cleanup after setup failure",
                      "TeardownError body", "TeardownError late", "TeardownError nested", "TeardownError earlier"]
    assert len(result.errors) == 3 and not result.failures and not result.skipped
    for (holder, detail), (phase, end) in zip(
        result.errors,
        [(f"setUpClass ({__name__}.SetupError)", "RuntimeError: setup failed"),
         (f"setUpClass ({__name__}.SetupError)", "ValueError: cleanup failed"),
         (f"tearDownClass ({__name__}.TeardownError)", "RuntimeError: teardown failed")],
    ):
        assert holder.id() == phase and detail.rstrip().endswith(end)
        print(phase, end)
    print(result.testsRun, events)


if __name__ == "__main__":
    current()
