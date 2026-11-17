"""Check the scorecard's counting and evidence boundaries."""
import copy
import json
import unittest

import scorecard
from replay_stdlib import parse


class ScorecardTests(unittest.TestCase):
    def script(self):
        digest = "a" * 64
        run = {"outcome": "pass", "argv": ["python", "script.py"], "exit_code": 0, "stdout_sha256": digest, "stderr_sha256": digest, "effects_sha256": digest, "effects_manifest": "effects.json", "evidence": "run.log"}
        runs = {t: copy.deepcopy(run) for t in scorecard.TIERS}; runs["stock"]["version"] = "3.14.5"
        return {"schema": scorecard.SCHEMA, "kind": "script", "id": "script:original-command", "source": {"repo": "https://github.com/example/repo", "commit": "b" * 40, "path": "script.py", "sha256": digest}, "normalized_invocation": {"cwd": "$REPO", "argv": ["python", "script.py"], "env": {}}, "imre_revision": scorecard.COHORT["imre_revision"], "platform": scorecard.COHORT["platform"], "runs": runs, "comparisons": {t: {d: "exact" for d in scorecard.DIMENSIONS} for t in ("O0", "O3")}, "omission_reason": None, "paired": True}

    def test_script_pair_requires_exact_observations(self):
        r = self.script(); scorecard.validate_record(r, "script", {})
        r["runs"]["O3"]["stdout_sha256"] = "c" * 64
        with self.assertRaisesRegex(ValueError, "contradicts observations"): scorecard.validate_record(r, "script", {})
        r["comparisons"]["O3"]["stdout"] = "different"; r["paired"] = False
        scorecard.validate_record(r, "script", {})

    def test_non_stock_success_and_skips_do_not_pair(self):
        for outcome in ("skip", "error", "timeout", "not_run", "omitted"):
            r = self.script(); r["runs"]["stock"].update(outcome=outcome, reason="not available")
            if outcome == "omitted": r["omission_reason"] = "explicit omission"
            with self.assertRaisesRegex(ValueError, "paired does not match"): scorecard.validate_record(r, "script", {})
            r["paired"] = False; scorecard.validate_record(r, "script", {})

    def test_pilot_and_unreplayed_denominators(self):
        fs, scripts = scorecard.summary()
        self.assertEqual((fs["errno"]["originals"], fs["errno"]["paired"], fs["errno"]["receipts"], fs["errno"]["cohort"]), (3, 3, 3, "supplemental"))
        frozen = [r for r in fs.values() if r["cohort"] == "frozen"]
        self.assertEqual((len(frozen), sum(r["originals"] for r in frozen)), (45, 16225))
        for view in ("readme", "stdlib"):
            rendered = scorecard.render(view)
            self.assertIn(f"Frozen 45-family cohort: **{sum(r['receipts'] for r in frozen):,} pinned receipts** of **16,225 raw upstream originals**", rendered)
            self.assertIn("Supplemental pilots, outside the frozen cohort: **3 receipts** of **3 originals**, **3 verified paired passes**", rendered)
        self.assertEqual((fs["argparse"]["originals"], fs["argparse"]["historical"]), (1893, 1887))
        readme_row = next(line for line in scorecard.render("readme").splitlines() if line.startswith("| `argparse`"))
        self.assertIn(readme_row, scorecard.render("stdlib").splitlines())
        if not fs["argparse"]["receipts"]:
            self.assertEqual(fs["argparse"]["runs"]["stock"]["not_run"], 1893)
            self.assertIn("unknown (not run)", readme_row)

    def test_unparsed_or_incomplete_unittest_output_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "unparsed unittest output"):
            parse(b"Ran 4 tests in 0.001s\n", "test.test_errno")

    def test_pilot_evidence_is_verified(self):
        r = json.loads((scorecard.HERE / "stdlib/errno.jsonl").read_text().splitlines()[0]); r["runs"]["O0"]["evidence"] = r["runs"]["O0"]["evidence"].replace("#L1", "#L3")
        with self.assertRaisesRegex(ValueError, "does not establish exact ID, occurrence and outcome"): scorecard.verify_stdlib_evidence(r)

    def test_colliding_class_method_suffix_is_not_the_original_id(self):
        r = json.loads((scorecard.HERE / "stdlib/errno.jsonl").read_text().splitlines()[0])
        r["id"] = r["id"].replace("test.test_errno.", "test.test_other.")
        with self.assertRaisesRegex(ValueError, "does not establish exact ID"):
            scorecard.verify_stdlib_evidence(r)

    def test_occurrence_must_match_the_raw_line(self):
        r = json.loads((scorecard.HERE / "stdlib/errno.jsonl").read_text().splitlines()[0])
        r["occurrence"] = 2
        with self.assertRaisesRegex(ValueError, "does not establish exact ID, occurrence"):
            scorecard.verify_stdlib_evidence(r)


if __name__ == "__main__": unittest.main()
