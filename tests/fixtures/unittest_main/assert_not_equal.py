import unittest


class NotEqualTest(unittest.TestCase):
    def test_arbitrary_operands(self):
        self.assertNotEqual("a", b"a")
        self.assertNotEqual(["a"], ["b"])

    def test_failure_message(self):
        with self.assertRaises(AssertionError) as caught:
            self.assertNotEqual("same", "same", "detail")
        self.assertEqual(str(caught.exception), "'same' == 'same' : detail")

    def test_arbitrary_short_message(self):
        self.longMessage = False
        marker = ["detail"]
        with self.assertRaises(AssertionError) as caught:
            self.assertNotEqual("same", "same", marker)
        self.assertIs(caught.exception.args[0], marker)


unittest.main()
