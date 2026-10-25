import unittest


def inspect_local() -> None:
    class Parent(unittest.TestCase):
        def test_parent(self):
            raise RuntimeError("discovery called an inherited method")
        def test_shadow(self):
            raise RuntimeError("discovery called a shadowed method")

    class Child(Parent):
        test_shadow = 42
        def test_child(self):
            raise RuntimeError("discovery called a child method")
        def other(self):
            raise RuntimeError("discovery called a prefixed method")

    print(Child.__name__ + " " + Child.__qualname__ + " " + Child.__module__)
    loader = unittest.TestLoader()
    print(loader.getTestCaseNames(Child))
    loader.testNamePatterns = ["__main__.inspect_local.<locals>.Child.test_parent"]
    print(loader.getTestCaseNames(Child))
    loader.testNamePatterns = ["__main__.Child.test_parent"]
    print(loader.getTestCaseNames(Child))
    loader.testMethodPrefix = "other"
    loader.testNamePatterns = None
    print(loader.getTestCaseNames(Child))


inspect_local()
