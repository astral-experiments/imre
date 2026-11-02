import unittest


def reason(label: str) -> str:
    print(label)
    return label


class Example(unittest.TestCase):
    @unittest.skipUnless(False, reason("missing feature"))
    def test_unavailable(self):
        self.fail("skipped body ran")

    @unittest.skipIf(False, reason("available"))
    def test_direct(self):
        with self.assertRaises(unittest.SkipTest) as caught:
            self.test_unavailable()
        self.assertEqual(str(caught.exception), "missing feature")
        self.assertIn(container=str(caught.exception), member="feature")
        with self.assertRaises(AssertionError):
            self.assertIn("available", str(caught.exception))


class Inherited(Example):
    pass


if __name__ == "__main__":
    unittest.main()
