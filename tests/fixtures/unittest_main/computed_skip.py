import os
import unittest


class Credential(unittest.TestCase):
    @unittest.skipUnless((print("callee") or hasattr)(os, "getuid"), "requires getuid")
    def test_uid(self):
        print("entered uid")

    @unittest.skipUnless(
        (print("missing") or hasattr)(os, "__unavailable_skip_feature__"),
        "requires missing feature",
    )
    def test_missing(self):
        self.fail("skipped body ran")


if __name__ == "__main__":
    unittest.main()
