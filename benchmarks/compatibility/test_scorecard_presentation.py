"""Presentation boundaries for canonical and explicitly selected snapshot views."""
from collections import Counter
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import scorecard

PIN = scorecard.REVISION_959


def row(originals, receipts=0, paired=0, pending=0):
    return {"cohort": "frozen", "originals": originals, "receipts": receipts, "observed": paired, "paired": paired, "pending": pending, "historical": None, "runs": {tier: Counter({"not_run": originals}) for tier in scorecard.TIERS}}


class PresentationTests(unittest.TestCase):
    def test_snapshot_missing_receipt_wording_does_not_change_canonical(self):
        with patch.object(scorecard, "summary", return_value=({"pending": row(4, pending=2)}, [])):
            with patch.object(scorecard, "is_snapshot", return_value=False):
                self.assertIn("4 originals have no receipt", scorecard.render("readme"))
            with patch.object(scorecard, "is_snapshot", return_value=True):
                rendered = scorecard.render("readme")
                self.assertIn("4 originals have no validated receipt", rendered)
                self.assertIn("pending (2 staged, unverified)", rendered)

    def test_root_summary_shows_only_staged_modules_and_keeps_pending_uncredited(self):
        fs = {"active": row(10, receipts=10, paired=8), "pending": row(20, pending=3), "untouched": row(30)}
        with patch.object(scorecard, "is_snapshot", return_value=True), patch.dict(scorecard.COHORT, {"imre_revision": PIN}):
            rendered = scorecard.render_root_summary(fs, [])
        self.assertIn("| `active` | 8 / 10 | validated |", rendered)
        self.assertIn("| `pending` | 0 / 20 | 3 staged, unverified |", rendered)
        self.assertIn("0 verified / 72 historically reported stock-success commands", rendered)
        self.assertIn("84 original script IDs", rendered)
        self.assertNotIn("untouched", rendered)
        self.assertIn(f"revisions/{PIN}/docs/stdlib.md", rendered)

    def test_root_selection_is_opt_in_and_check_scopes_are_independent(self):
        fs = {"active": row(10, receipts=10, paired=8)}
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); data = root / "revision"; data.mkdir(); (root / "docs").mkdir()
            block = scorecard.START + "\ncontent\n" + scorecard.END
            stale = scorecard.ROOT_SUMMARY_START + "\nstale\n" + scorecard.ROOT_SUMMARY_END
            original = block + "\n\n" + stale + "\n"
            (root / "README.md").write_text(original); (root / "docs/stdlib.md").write_text(block + "\n")
            with patch.object(scorecard, "ROOT", root), patch.object(scorecard, "DATA_ROOT", data), patch.dict(scorecard.COHORT, {"imre_revision": PIN}), patch.object(scorecard, "summary", return_value=(fs, [])), patch.object(scorecard, "render", return_value=block), patch.object(scorecard, "script_cohort_text", return_value="fixture script cohort"), patch.object(scorecard, "select_snapshot"), patch("check_omissions.validate"):
                with patch.object(scorecard, "is_snapshot", return_value=False), patch.object(sys, "argv", ["scorecard.py", "--check"]):
                    scorecard.main()  # The canonical check ignores the separate stale marker.
                with patch.object(scorecard, "is_snapshot", return_value=True):
                    with patch.object(sys, "argv", ["scorecard.py", "--snapshot", PIN]): scorecard.main()
                    self.assertEqual((root / "README.md").read_text(), original)
                    with patch.object(sys, "argv", ["scorecard.py", "--snapshot", PIN, "--check"]): scorecard.main()
                    with patch.object(sys, "argv", ["scorecard.py", "--snapshot", PIN, "--root-summary", "--check"]):
                        with self.assertRaisesRegex(ValueError, "stale current snapshot summary"): scorecard.main()
                    with patch.object(sys, "argv", ["scorecard.py", "--snapshot", PIN, "--root-summary"]): scorecard.main()
                    with patch.object(sys, "argv", ["scorecard.py", "--snapshot", PIN, "--root-summary", "--check"]): scorecard.main()
                    self.assertTrue((root / "README.md").read_text().startswith(block + "\n\n"))

    def test_root_summary_requires_explicit_snapshot(self):
        with patch.object(sys, "argv", ["scorecard.py", "--root-summary"]), redirect_stderr(StringIO()):
            with self.assertRaises(SystemExit) as exc: scorecard.main()
        self.assertEqual(exc.exception.code, 2)


if __name__ == "__main__": unittest.main()
