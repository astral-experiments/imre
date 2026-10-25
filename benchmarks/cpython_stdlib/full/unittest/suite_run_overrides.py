"""A suite calls an override for its effects and propagates argument errors."""
import unittest


class Records(unittest.TestCase):
    def run(self, result: unittest.TestResult) -> None:
        result.startTest(self)
        print("custom run")
        result.stopTest(self)

    def test_body(self) -> None:
        print("base run selected")


class Wrong(unittest.TestCase):
    def run(self) -> None:
        print("entered a bad call")

    def test_body(self) -> None:
        pass


result = unittest.TestResult()
unittest.TestSuite([Records("test_body")]).run(result)
try:
    unittest.TestSuite([Wrong("test_body")]).run(result)
except TypeError as exc:
    print(str(exc))
print(result.testsRun)
