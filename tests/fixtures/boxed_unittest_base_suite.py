import unittest
from unittest.suite import BaseTestSuite, TestSuite


events = []


def setUpModule():
    events.append("module setup")


class Example(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        events.append("class setup")

    def test_body(self):
        events.append("body")


class RoutedSuite(BaseTestSuite):
    def run(self, result, *, tag):
        events.append(tag)
        return super().run(result)

    def debug(self):
        events.append("debug")
        super().debug()


def current():
    assert unittest.BaseTestSuite is BaseTestSuite
    assert unittest.TestSuite is TestSuite
    assert unittest.defaultTestLoader.suiteClass is TestSuite
    assert TestSuite.__bases__ == (BaseTestSuite,)

    case = Example("test_body")
    suite = RoutedSuite([case])
    suite._cleanup = False
    result = unittest.TestResult()
    events.clear()
    assert suite(result=result, tag="run") is result
    assert result.wasSuccessful() and result.testsRun == 1
    assert list(suite) == [case]

    suite._cleanup = True
    BaseTestSuite([suite]).debug()
    assert list(suite) == [case]
    assert events == ["run", "body", "debug", "body"]

    def stop(result):
        result.stop()

    interrupted = BaseTestSuite([stop, case])
    assert interrupted(result) is result
    assert list(interrupted) == [None, case]
    assert interrupted.countTestCases() == 1
    assert result.testsRun == 1
