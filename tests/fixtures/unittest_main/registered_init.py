import unittest


class Prebuilt(unittest.TestCase):
    def __init__(self, value: int):
        super().__init__("test_value")
        self.value = value

    def test_value(self):
        self.value += 1


case = Prebuilt(41)
suite = unittest.TestSuite()
suite.addTest(case)
result = suite.run(unittest.TestResult())
assert result.testsRun == 1 and result.wasSuccessful()
assert case.value == 42
