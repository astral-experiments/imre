import io
import pickle
import unittest


def current():
    events = []

    class Result(unittest.TestResult):
        def addDuration(self, test, elapsed):
            events.append("duration")
            super().addDuration(test, elapsed)

        def addSuccess(self, test):
            events.append("success")
            super().addSuccess(test)

    class Test(unittest.TestCase):
        def test_body(self):
            self.addCleanup(events.append, "cleanup")

    test = Test("test_body")
    result = Result()
    test.run(result)
    assert events == ["cleanup", "duration", "success"]
    [(description, elapsed)] = result.collectedDurations
    assert description == str(test) and elapsed >= 0

    result.collectedDurations = [("hidden", 0), ("slower", 2), ("faster", 1)]
    stream = io.StringIO()
    runner = unittest.TextTestRunner(stream=stream, durations=2)
    runner._printDurations(result)
    text = stream.getvalue()
    assert pickle.loads(pickle.dumps(runner)).stream.getvalue() == text
    assert 0 <= text.find("slower") < text.find("faster") and "hidden" not in text

    class Writer:
        def write(self, text, *, tag=None):
            return (text, tag)

    writer = Writer()
    wrapped = unittest.TextTestRunner(stream=writer).stream
    nested = unittest.TextTestRunner(stream=wrapped).stream
    assert not hasattr(type(wrapped), "write")
    assert nested.write == writer.write
    assert nested.write("test", tag=7) == ("test", 7)
    wrapped.write = lambda text: "override"
    assert nested.write("test") == "override"
    del wrapped.write
    assert nested.write == writer.write
