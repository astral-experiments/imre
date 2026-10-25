from copy import deepcopy
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from native_report import rollup, verified_counts
from suite_evidence import build_report


class PairedCredit(unittest.TestCase):
    def setUp(self):
        self.ref = {"module": "test.example", "cpython_commit": "a" * 40, "platform": "linux",
                    "denominator": 2, "reference": {"repeated": ["pass", "pass"]}}
        discovery = [("repeated", "first fixture"), ("repeated", "second fixture")]
        observed = {"outcome": "pass", "roles": dict.fromkeys(("body", "assertions", "operation", "fixture"), "imre"),
                    "evidence": {"operation": "native operation", "fixture": "native setup"}}
        self.pair = [build_report(self.ref, level, discovery, {("repeated", i): observed})
                     for level, i in ((0, 1), (3, 2))]

    def test_different_occurrences_do_not_earn_paired_credit(self):
        both, levels = verified_counts(self.pair, self.ref)
        self.assertEqual([levels[v]["fully-native"] for v in ("O0", "O3")], [1, 1])
        self.assertEqual(both, 0)

    def test_fixture_variant_cannot_change_between_levels(self):
        pair = deepcopy(self.pair)
        pair[1]["records"][0]["fixture_variant"] = "different fixture"
        with self.assertRaisesRegex(ValueError, "fixture variant"):
            verified_counts(pair, self.ref)

    def test_legacy_results_and_unmatched_ledgers_never_earn_credit(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            module = root / "example"
            module.mkdir()
            (root / "scope.toml").write_text('[wave_one]\nexample = "example"\n')
            (module / "reference-linux.json").write_text(json.dumps(self.ref))
            (module / "native-linux.json").write_text(json.dumps({"native_calls": 10}))
            row = rollup(root, self.ref["cpython_commit"], "linux")["example"]
            self.assertEqual((row["instances"], row["ledger_instances"], row["fully_native_both"]), (2, 0, None))
            (module / "evidence-other-linux.json").write_text(json.dumps(self.pair))
            with self.assertRaisesRegex(ValueError, "no matching reference"):
                rollup(root, self.ref["cpython_commit"], "linux")


if __name__ == "__main__":
    unittest.main()
