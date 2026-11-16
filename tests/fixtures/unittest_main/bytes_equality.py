import unittest


class Bytes(unittest.TestCase):
    def test_bytes_and_text(self):
        self.assertEqual(b"x", b"x")
        with self.assertRaises(AssertionError) as left:
            self.assertEqual(b"x", "x", msg="left")
        self.assertEqual(str(left.exception), "b'x' != 'x' : left")
        with self.assertRaises(AssertionError) as right:
            self.assertEqual("x", b"x")
        self.assertEqual(str(right.exception), "'x' != b'x'")


if __name__ == "__main__":
    unittest.main()
