import io
import unittest


class Case(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__("test_second")

    def test_first(self) -> None:
        print("first body")

    def test_second(self) -> None:
        print("second body")


loader = unittest.TestLoader()
loader.testNamePatterns = ["*.test_first"]
suite = loader.loadTestsFromTestCase(Case)
stream = io.StringIO()
result = unittest.TextTestRunner(stream=stream, verbosity=2).run(suite)
print(result.testsRun)
print(stream.getvalue().splitlines()[0])
