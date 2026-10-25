"""Exercise discovery on a pinned upstream class and fixed user classes."""
from bar.test_bar import PassingTest
from unittest import TestCase, TestLoader, defaultTestLoader


class Base(TestCase):
    def test_z(self) -> None:
        raise AssertionError("discovery ran a test")

    def test_inherited(self) -> None:
        raise AssertionError("discovery ran an inherited test")

    def check_prefix(self) -> None:
        raise AssertionError("discovery called a method")


class Derived(Base):
    test_constant = "not callable"

    @staticmethod
    def test_static() -> None:
        raise AssertionError("discovery called a static method")

    @property
    def test_property(self) -> str:
        raise AssertionError("discovery ran an instance getter")

    def test_z(self) -> None:
        raise AssertionError("discovery ran an override")


class Plain:
    def __init__(self) -> None:
        pass

    def test_plain(self) -> None:
        raise AssertionError("discovery called a plain-class method")


loader = TestLoader()
print(loader.testMethodPrefix)
print(loader.testNamePatterns is None)
loader.errors.append("local error")
print(defaultTestLoader.errors)
print(defaultTestLoader.getTestCaseNames(PassingTest))
print(loader.getTestCaseNames(testCaseClass=Derived))
print(loader.getTestCaseNames(Plain))
loader.testNamePatterns = []
print(loader.getTestCaseNames(Derived))
loader.testNamePatterns = ["__main__.Derived.test_[si]*", "*.test_static"]
print(loader.getTestCaseNames(Derived))
loader.testNamePatterns = ["__main__.derived.*"]
print(loader.getTestCaseNames(Derived))
loader.testNamePatterns = ["bar.test_bar.PassingTest.test_true"]
print(loader.getTestCaseNames(PassingTest))
loader.testNamePatterns = None
loader.testMethodPrefix = "check"
print(loader.getTestCaseNames(Derived))
