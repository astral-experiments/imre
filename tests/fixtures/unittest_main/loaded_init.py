import unittest


def chosen(name: str) -> str:
    print("constructor " + name)
    if name == "test_trap":
        raise ValueError(name)
    return name


class Parent(unittest.TestCase):
    selected: str
    suffix: str

    def __init__(self, methodName: str, suffix: str = "!"):
        super().__init__(methodName)
        self.selected = chosen(methodName)
        self.suffix = suffix


class Child(Parent):
    def test_selected(self):
        self.assertEqual(self.selected + self.suffix, "test_selected!")

    def test_trap(self):
        raise AssertionError("a failed constructor cannot yield a case")


loader = unittest.TestLoader()
loader.testNamePatterns = ["*.test_selected"]
suite = loader.loadTestsFromTestCase(Child)
result = suite.run(unittest.TestResult())
assert result.testsRun == 1 and result.wasSuccessful()
loader.testNamePatterns = ["*.test_trap"]
try:
    loader.loadTestsFromTestCase(Child)
except ValueError as error:
    assert str(error) == "test_trap"
else:
    raise AssertionError("constructor failure was ignored")
