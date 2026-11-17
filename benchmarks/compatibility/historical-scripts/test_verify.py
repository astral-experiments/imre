"""Focused controls for the historical 84-command join."""
import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import verify


class JoinTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.inputs=verify.load()

    def mutate(self, side, id, change, message):
        inputs=copy.deepcopy(self.inputs);change(inputs[side][id])
        with self.assertRaisesRegex(ValueError,message): verify.derive(*inputs)

    def test_exact_join_and_verdicts(self):
        rows=verify.derive(*self.inputs)
        self.assertEqual(len(rows),84)
        self.assertEqual(sum(r["archived_reported_stock_exit"]==0 for r in rows),72)
        self.assertEqual(sum(r["canonical_stock_outcome"]=="pass" for r in rows),52)
        self.assertEqual(sum(r["independent_frozen_entry"] for r in rows),7)
        self.assertEqual(verify.encoded(rows),(verify.HERE/"join.jsonl").read_text())

    def test_source_path_and_sha(self):
        self.mutate(1,"uv-01",lambda r:r["source"].update(path="wrong.py"),"source path or SHA")
        self.mutate(1,"uv-01",lambda r:r["source"].update(sha256="0"*64),"source path or SHA")

    def test_checkout_and_ty_file_commits(self):
        self.mutate(0,"uv-01",lambda r:r.update(git_revision="0"*40),"commit")
        self.mutate(1,"ty-02",lambda r:r["source"].update(commit=verify.CHECKOUT["ty"]),"commit")

    def test_exact_historical_argv(self):
        self.mutate(1,"ruff-03",lambda r:r["original_argv"].append("--different"),"historical argv")

    def test_uv_role_and_unresolved_mapping(self):
        self.mutate(1,"uv-03",lambda r:r["normalized_invocation"]["argv"].__setitem__(4,"$VENV/bin/uv"),"argv structure|role mapping")
        self.mutate(1,"uv-24",lambda r:r["normalized_invocation"]["argv"].__setitem__(4,"$UV19"),"unresolved mapping")

    def test_reported_and_latest_stock_verdicts(self):
        self.mutate(0,"uv-13",lambda r:r.update(stock_O0="0"),"archived stock")
        self.mutate(1,"uv-27",lambda r:r["runs"]["stock"].update(outcome="fail"),"canonical stock transition")

    def test_join_count_and_duplicate(self):
        inputs=copy.deepcopy(self.inputs);del inputs[1]["uv-01"]
        with self.assertRaisesRegex(ValueError,"join count"):verify.derive(*inputs)
        with self.assertRaisesRegex(ValueError,"duplicate"):verify.unique([{"id":"x"},{"id":"x"}])

    def test_reviewed_anchors_survive_unrelated_addition(self):
        raw = (verify.COMPAT/"frozen/scripts.jsonl").read_bytes()
        extra = json.dumps({"id": "future-anchor"}).encode() + bytes([10])
        self.assertEqual(verify.reviewed_frozen(raw + extra), self.inputs[2])
        altered = raw.replace(b'"uv-10"', b'"uv-10" ', 1)
        with self.assertRaisesRegex(ValueError, "reviewed frozen anchor differs"):
            verify.reviewed_frozen(altered)

    def test_rebound_archive_digest_is_not_authority(self):
        raw=(verify.HERE/"archived-survey-7724.tsv").read_bytes().replace(b"uv-01",b"uv-XX",1)
        self.assertNotEqual(hashlib.sha256(raw).hexdigest(),verify.ARCHIVE_SHA)
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/"survey.tsv";path.write_bytes(raw)
            with self.assertRaisesRegex(ValueError,"source digest differs"):
                verify.checked(path,verify.ARCHIVE_SHA)


if __name__=="__main__":unittest.main()
