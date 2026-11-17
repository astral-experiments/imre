"""Distinguish the lost-hook RuntimeError from an unlinked-code refusal."""
import sys
import unittest


class NativeCase(unittest.TestCase):
    def test_lost_displayhook(self):
        displayhook = sys.displayhook
        try:
            del sys.displayhook
            code = compile("42", "<string>", "single")
            self.assertRaisesRegex(RuntimeError, "^lost sys.displayhook$", eval, code)
        finally:
            sys.displayhook = displayhook


NativeCase().test_lost_displayhook()
