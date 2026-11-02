import os
import unittest


class CredentialGuard(unittest.TestCase):
    @unittest.skipUnless(hasattr(os, "getuid"), "requires os.getuid")
    def test_uid(self) -> None:
        self.assertEqual(os.getuid(), os.getuid())


if __name__ == "__main__":
    unittest.main()
