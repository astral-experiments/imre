"""Revision isolation and credit boundaries for compatibility snapshots."""
import copy
import hashlib
import json
import tempfile
import os
from contextlib import contextmanager
import unittest
import sys
from pathlib import Path
from unittest.mock import patch

import scorecard

PIN = "8571ca2dbecabf8d7863c369dc776611fd57d8ad"


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix=".scorecard-snapshot-test-", dir=scorecard.ROOT)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.cohort = {**scorecard.CANONICAL_COHORT, "imre_revision": PIN}
        self.addCleanup(patch.stopall)
        patch.object(scorecard, "DATA_ROOT", self.root).start()
        patch.object(scorecard, "COHORT", self.cohort).start()

    def locator(self, path): return path.relative_to(scorecard.ROOT).as_posix()

    def ref(self, name, obj):
        path = self.root / name; data = (json.dumps(obj, sort_keys=True) + "\n").encode(); path.write_bytes(data)
        return {"path": self.locator(path), "sha256": hashlib.sha256(data).hexdigest()}

    def receipt(self):
        r = copy.deepcopy(json.loads((scorecard.HERE / "stdlib/errno.jsonl").read_text().splitlines()[0]))
        r.update(imre_revision=PIN, launcher_id="errno-launcher", paired=False, omission_reason=None, credit_reason="control pending")
        stock = r["runs"]["stock"]; path = scorecard.ROOT / stock["evidence"].split("#L")[0]
        stock["evidence_provenance"] = {"kind": "immutable_historical_cpython_3.14.5", "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        tools = {"compiler_revision": PIN, "runtime_revision": PIN, "compiler_sha256": "a" * 64, "runtime_sha256": "b" * 64}
        r["toolchain_evidence"] = self.ref("build.json", {"source_revision": PIN, "clean_source": True, "toolchain": tools, "build_argv": ["cargo", "build", "--release"], "artifacts": {"imre": tools["compiler_sha256"], "runtime.a": tools["runtime_sha256"]}})
        for tier in ("O0", "O3"):
            run = r["runs"][tier]; old, line = run["evidence"].split("#L"); p = self.root / (tier + ".stderr"); p.write_bytes((scorecard.ROOT / old).read_bytes())
            run.update(evidence=self.locator(p) + "#L" + line, toolchain=copy.deepcopy(tools), launcher_id=r["launcher_id"], binary_sha256=("c" if tier == "O0" else "d") * 64, build_argv=["imre", "build", "launcher.py", "--opt-level", tier[1:], "--runtime", "runtime.a"])
        return r

    def control(self, r, passed=True):
        stderr = "expected int, got bool" if passed else ""
        source = self.root / "poison.py"; source.write_text("status: int = True\n")
        positive = self.root / "launcher.py"; positive.write_text("print(1)\n")
        poisoned = self.root / "poisoned_launcher.py"; poisoned.write_text("import poison\nprint(1)\n")
        poison = {"positive_launcher_path": self.locator(positive), "positive_launcher_sha256": hashlib.sha256(positive.read_bytes()).hexdigest(), "poisoned_launcher_path": self.locator(poisoned), "poisoned_launcher_sha256": hashlib.sha256(poisoned.read_bytes()).hexdigest(), "source_path": self.locator(source), "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(), "imported_source": "status: int = True", "imported_module": "poison", "launcher_id": r["launcher_id"], "argv": ["imre", "build", "launcher.py", "--no-cache", "--opt-level", "0", "--runtime", "runtime.a"], "exit_code": 1 if passed else 0, "image_created": not passed, "stderr": stderr, "stderr_sha256": hashlib.sha256(stderr.encode()).hexdigest()}
        saved = {}
        for tier in ("O0", "O3"):
            evidence, line = r["runs"][tier]["evidence"].split("#L"); data = (scorecard.ROOT / evidence).read_bytes()
            saved[tier] = self.ref(tier + "-saved.json", {"outcome_evidence": {"path": evidence, "sha256": hashlib.sha256(data).hexdigest(), "line": int(line)}, "imre_revision": PIN, "launcher_id": r["launcher_id"], "tier": tier, "binary_sha256": r["runs"][tier]["binary_sha256"], "source_withdrawn": True, "child_exec_observed": False, "libpython_loaded": False})
        return self.ref("control.json", {"schema": "imre.compatibility.launcher-control.v1", "imre_revision": PIN, "launcher_id": r["launcher_id"], "toolchain": r["runs"]["O0"]["toolchain"], "poison": poison, "saved_checks": saved})

    def validate(self, r):
        scorecard.validate_record(r, "stdlib", scorecard.families())
        scorecard.verify_stdlib_evidence(r)

    def test_selector_pins_cohort_and_keeps_repository_root(self):
        canonical_root = scorecard.ROOT
        with patch.object(scorecard, "HERE", self.root):
            root = self.root / "revisions" / PIN; root.mkdir(parents=True); (root / "cohort.json").write_text(json.dumps(self.cohort))
            scorecard.select_snapshot(PIN)
            self.assertEqual((scorecard.DATA_ROOT, scorecard.ROOT), (root, canonical_root))
            with self.assertRaisesRegex(ValueError, "40-hex"): scorecard.select_snapshot("../" + PIN)
            (root / "cohort.json").write_text(json.dumps({**self.cohort, "imre_revision": "0" * 40}))
            with self.assertRaisesRegex(ValueError, "cohort"): scorecard.select_snapshot(PIN)

    def test_missing_argparse_stays_not_run_without_receipt_synthesis(self):
        fs, scripts = scorecard.summary()
        self.assertEqual((fs["argparse"]["originals"], fs["argparse"]["receipts"], fs["argparse"]["runs"]["stock"]["not_run"], scripts), (1893, 0, 1893, []))

    def test_snapshot_render_never_writes_canonical_views_or_synthesizes_receipts(self):
        canonical = (scorecard.ROOT / "README.md").read_bytes()
        with patch.object(sys, "argv", ["scorecard.py", "--snapshot", PIN]), patch.object(scorecard, "select_snapshot"), patch("check_omissions.validate"):
            with self.assertRaisesRegex(ValueError, "without genuine receipts"): scorecard.main()
            self.assertFalse((self.root / "README.md").exists())
            row = self.receipt(); directory = self.root / "stdlib"; directory.mkdir(); (directory / "errno.jsonl").write_text(json.dumps(row) + "\n")
            scorecard.main()
            self.assertIn("0 verified credited pairs", (self.root / "README.md").read_text())
            self.assertTrue((self.root / "docs/stdlib.md").is_file())
            with patch.object(sys, "argv", ["scorecard.py", "--snapshot", PIN, "--check"]): scorecard.main()
        self.assertEqual((scorecard.ROOT / "README.md").read_bytes(), canonical)

    def test_historical_or_fresh_stock_and_exact_original(self):
        r = self.receipt(); self.validate(r)
        r["runs"]["stock"].pop("evidence_provenance")
        with self.assertRaisesRegex(ValueError, "historical stock"): self.validate(r)
        stock = r["runs"]["stock"]; old, line = stock["evidence"].split("#L"); p = self.root / "fresh.stderr"; p.write_bytes((scorecard.ROOT / old).read_bytes()); stock["evidence"] = self.locator(p) + "#L" + line
        self.validate(r)
        r["occurrence"] = 2
        with self.assertRaisesRegex(ValueError, "original ID, occurrence"): self.validate(r)

    def test_old_native_evidence_and_mixed_runtime_are_rejected(self):
        r = self.receipt(); r["runs"]["O0"]["evidence"] = "benchmarks/compatibility/evidence/errno/O0.stderr#L1"
        with self.assertRaisesRegex(ValueError, "outside allowed root"): self.validate(r)
        r = self.receipt(); r["runs"]["O3"]["toolchain"]["runtime_revision"] = "0" * 40
        with self.assertRaisesRegex(ValueError, "compiler/runtime revision"): self.validate(r)

    def test_observed_triple_needs_control_to_earn_credit(self):
        r = self.receipt(); self.validate(r); row = {"originals": 1}; scorecard.summarize_family(row, [r]); self.assertEqual((row["observed"], row["paired"]), (1, 0))
        r["paired"] = True
        with self.assertRaisesRegex(ValueError, "passing launcher control"): self.validate(r)
        r["launcher_control"] = self.control(r); self.validate(r)
        r["launcher_control"] = self.control(r, passed=False)
        with self.assertRaisesRegex(ValueError, "passing launcher control"): self.validate(r)
        r["paired"] = False; self.validate(r)
        r["omission_reason"] = "accepted failed control"
        with self.assertRaisesRegex(ValueError, "canonical policy omission"): self.validate(r)

    def test_control_digest_and_saved_image_checks_are_enforced(self):
        r = self.receipt(); r["paired"] = True; r["launcher_control"] = self.control(r)
        (self.root / "O3-saved.json").write_text("{}")
        with self.assertRaisesRegex(ValueError, "digest differs"): self.validate(r)
        r["launcher_control"] = self.control(r); r["runs"]["O3"]["binary_sha256"] = "e" * 64
        with self.assertRaisesRegex(ValueError, "binary identity"): self.validate(r)

    def test_script_uses_canonical_frozen_anchor_and_historical_stock_line(self):
        path = scorecard.HERE / "scripts/uv-a.jsonl"; lines = path.read_text().splitlines()
        line, r = next((i, json.loads(s)) for i, s in enumerate(lines, 1) if json.loads(s)["id"] == "uv-10")
        template = self.receipt(); r.update(imre_revision=PIN, paired=False, credit_reason="control pending", launcher_id=template["launcher_id"], toolchain_evidence=template["toolchain_evidence"])
        r["runs"]["stock"].update(evidence=self.locator(path) + "#L" + str(line), evidence_provenance={"kind": "immutable_historical_cpython_3.14.5", "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
        for tier in ("O0", "O3"):
            evidence = self.root / (tier + "-script.jsonl"); evidence.write_text(json.dumps({"id": r["id"], "tier": tier, "outcome": "pass"}) + "\n")
            r["runs"][tier].update({k: copy.deepcopy(template["runs"][tier][k]) for k in ("toolchain", "launcher_id", "binary_sha256", "build_argv")})
            r["runs"][tier]["evidence"] = self.locator(evidence) + "#L1"
        scorecard.validate_record(r, "script", scorecard.families(), scorecard.load_frozen_scripts())
        r["runs"]["O0"]["stdout_path"] = self.locator(path); r["runs"]["O0"].pop("stdout", None); r["runs"]["O0"]["stdout_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
        with self.assertRaises(ValueError): scorecard.validate_record(r, "script", scorecard.families(), scorecard.load_frozen_scripts())

    def test_wrong_source_or_raw_line_cannot_establish_original(self):
        r = self.receipt(); r["source"]["sha256"] = "f" * 64
        with self.assertRaisesRegex(ValueError, "source differs"): self.validate(r)
        r = self.receipt(); r["runs"]["O0"]["evidence"] = r["runs"]["O0"]["evidence"].replace("#L1", "#L3")
        with self.assertRaisesRegex(ValueError, "exact ID, occurrence"): self.validate(r)


@unittest.skipUnless(os.environ.get("SCORECARD_SHARD_NEGATIVES") == "1", "stage the published revision shards to run integration checks")
class PublishedShardTests(unittest.TestCase):
    """Run against the unmodified published shards in an ephemeral checkout."""
    def setUp(self):
        self.addCleanup(patch.stopall)
        self.root = scorecard.HERE / "revisions" / PIN
        patch.object(scorecard, "DATA_ROOT", self.root).start()
        patch.object(scorecard, "COHORT", {**scorecard.CANONICAL_COHORT, "imre_revision": PIN}).start()
        scorecard.adapt_family_replay.cache_clear()

    def row(self, family):
        return json.loads((self.root / "stdlib" / f"{family}.jsonl").read_text().splitlines()[0])

    @contextmanager
    def change(self, path, transform):
        original = path.read_bytes()
        try:
            path.write_bytes(transform(original)); scorecard.adapt_family_replay.cache_clear()
            yield
        finally:
            path.write_bytes(original); scorecard.adapt_family_replay.cache_clear()

    def validate(self, row):
        scorecard.validate_record(row, "stdlib", scorecard.families())
        scorecard.verify_stdlib_evidence(row)

    def test_genuine_counts_and_no_argparse_synthesis(self):
        fs, scripts = scorecard.summary()
        self.assertEqual((fs["configparser"]["receipts"], fs["configparser"]["paired"], fs["fnmatch"]["receipts"], fs["fnmatch"]["paired"]), (344, 339, 24, 24))
        self.assertEqual((sum(f["receipts"] for f in fs.values()), sum(f["paired"] for f in fs.values()), fs["argparse"]["receipts"], len(scripts)), (368, 363, 0, 0))

    def test_nonvendored_source_and_occurrence_mismatch(self):
        row = self.row("configparser")
        self.assertFalse((scorecard.ROOT / "benchmarks/cpython_stdlib/full/configparser/upstream" / row["source"]["path"]).exists())
        row["source"]["sha256"] = "f" * 64
        with self.assertRaisesRegex(ValueError, "source differs"): self.validate(row)
        row = self.row("configparser"); row["occurrence"] += 1
        with self.assertRaisesRegex(ValueError, "original ID, occurrence"): self.validate(row)

    def test_old_native_evidence_is_rejected(self):
        row = self.row("configparser"); row["runs"]["O0"]["evidence"] = scorecard.canonical_original(row, "stdlib")["runs"]["O0"]["evidence"]
        with self.assertRaisesRegex(ValueError, "outside allowed root"): self.validate(row)

    def test_poisoned_copy_must_be_exact_import_only_change(self):
        directory = self.root / "evidence/fnmatch/architecture"; poisoned = directory / "poisoned_test_fnmatch.py"; control = directory / "control.json"
        changed = poisoned.read_bytes() + b"# unrelated change\n"
        def update(data):
            obj = json.loads(data); obj["poisoned_launcher_sha256"] = hashlib.sha256(changed).hexdigest(); return (json.dumps(obj) + "\n").encode()
        with self.change(poisoned, lambda _: changed), self.change(control, update):
            with self.assertRaisesRegex(ValueError, "copy or explicit import"): self.validate(self.row("fnmatch"))

    def test_poison_must_use_o0_and_matching_runtime(self):
        control = self.root / "evidence/fnmatch/architecture/control.json"
        for opt in ("--runtime", "--opt-level"):
            def update(data):
                obj = json.loads(data); argv = obj["graph_build"]["argv"]; argv[argv.index(opt)+1] = "old-runtime.a" if opt == "--runtime" else "3"; return (json.dumps(obj) + "\n").encode()
            with self.change(control, update):
                with self.assertRaisesRegex(ValueError, "O0 and the exact pinned compiler/runtime"): self.validate(self.row("fnmatch"))

    def test_missing_control_is_rejected(self):
        run = self.root / "evidence/fnmatch/run.json"
        def update(data):
            obj = json.loads(data); obj.pop("architecture_control"); return (json.dumps(obj) + "\n").encode()
        with self.change(run, update):
            with self.assertRaisesRegex(ValueError, "no referenced control"): self.validate(self.row("fnmatch"))


if __name__ == "__main__": unittest.main()
