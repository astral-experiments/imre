import unittest


def reason(label: str) -> str:
    print(label)
    return label


def suffix(label):
    print("evaluate", label)
    def decorate(method):
        print("apply", label)
        def wrapped(self):
            return method(self) + label
        return wrapped
    return decorate


class Example(unittest.TestCase):
    @property
    @suffix("outer")
    @suffix("inner")
    def value(self):
        return "start"

    @unittest.skipUnless(False, reason("missing feature"))
    def test_unavailable(self):
        self.fail("skipped body ran")

    @unittest.skipIf(False, reason("available"))
    def test_direct(self):
        self.assertEqual(self.value, "startinnerouter")
        with self.assertRaises(unittest.SkipTest) as caught:
            self.test_unavailable()
        self.assertEqual(str(caught.exception), "missing feature")


class Inherited(Example):
    pass


if __name__ == "__main__":
    unittest.main()
