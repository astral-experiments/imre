import unittest


def current():
    events = []

    class Observer:
        def startTest(self, test):
            events.append("start")

        def stopTest(self, test):
            events.append("stop")

        def addDuration(self, test, elapsed):
            pass

        def addSuccess(self, test):
            events.append("success")

    observer = Observer()

    class Case(unittest.TestCase):
        def defaultTestResult(self):
            events.append("factory")
            return observer

        def runTest(self):
            events.append("body")

    case = Case()
    assert type(unittest.TestCase().defaultTestResult()) is unittest.TestResult
    assert case.run() is observer
    assert events == ["factory", "start", "body", "success", "stop"]
    events.clear()
    observer.startTestRun = lambda: events.append("start run")
    observer.stopTestRun = lambda: events.append("stop run")
    assert case(None) is observer
    assert events == ["factory", "start run", "start", "body", "success", "stop", "stop run"]
    events.clear()
    assert case.run(observer) is observer
    assert events == ["start", "body", "success", "stop"]
