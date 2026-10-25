"""A modified or extra test file cannot enter the reference denominator unnoticed."""
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from discover_staged import early_skips
from stage_tests import stage, verify_prefix


class FixtureChecks(unittest.TestCase):
    def test_mixed_or_modified_test_installation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            count = stage(root)
            stdlib = root / "Lib"
            self.assertEqual(verify_prefix(stdlib), count)
            extra = stdlib / "test/test_unvendored.py"
            extra.touch()
            with self.assertRaisesRegex(ValueError, "unexpected installed fixture"):
                verify_prefix(stdlib)
            extra.unlink()
            (stdlib / "test/support/__init__.py").write_text("# changed upstream helper\n")
            with self.assertRaisesRegex(ValueError, "modified installed fixture"):
                verify_prefix(stdlib)

    def test_import_skip_does_not_mask_reference_passes(self):
        entries = [("test.suite.A.test_a", {"skipped": "GIL enabled"}), ("test.suite.B.test_b", "pass")]
        with self.assertRaisesRegex(ValueError, "test.suite.B.test_b"):
            early_skips("test.suite", "GIL enabled", entries)


if __name__ == "__main__":
    unittest.main()
