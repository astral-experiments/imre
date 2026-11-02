import unittest


def optional(value: str | None) -> str | None:
    return value


class Text(unittest.TestCase):
    def test_optional_text(self):
        self.assertEqual(optional("value"), "value")
        self.assertEqual("value", optional("value"))
        self.assertEqual(optional(None), optional(None))
        with self.assertRaises(AssertionError) as left:
            self.assertEqual(optional(None), "value", "missing")
        self.assertEqual(str(left.exception), "None != 'value' : missing")
        with self.assertRaises(AssertionError) as right:
            self.assertEqual("value", optional(None))
        self.assertEqual(str(right.exception), "'value' != None")
        with self.assertRaises(AssertionError) as different:
            self.assertEqual(optional("a"), optional("b"))
        self.assertIn("- a", str(different.exception))
        self.assertIn("+ b", str(different.exception))


if __name__ == "__main__":
    unittest.main()
