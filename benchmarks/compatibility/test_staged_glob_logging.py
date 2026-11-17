"""Bounded tests for the unactivated glob/logging stage audit."""
import copy
import json
import unittest

import audit_staged_glob_logging as audit


class StagedAuditTests(unittest.TestCase):
    def test_staged_triples_never_become_credit(self):
        result = audit.audit()
        self.assertEqual([len(v["candidate_intersections"]) for v in result["families"].values()], [21, 1])
        self.assertEqual(result["strict_pairs_added"], 0)
        self.assertEqual([v["validated_receipts"] for v in result["families"].values()], [0, 0])

    def test_exact_result_rejects_another_frozen_id(self):
        rows = [json.loads(line) for line in (audit.SNAPSHOT / "stdlib/glob.jsonl").read_text().splitlines()]
        changed = copy.deepcopy(rows[0])
        changed["id"] = rows[1]["id"]  # another valid frozen ID, but the original raw line remains
        with self.assertRaisesRegex(ValueError, "exact ID, occurrence"):
            audit.exact_result(changed, "stock")


if __name__ == "__main__":
    unittest.main()
