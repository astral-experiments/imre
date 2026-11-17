"""Revision isolation and credit boundaries for compatibility snapshots."""
import copy
import hashlib
import json
import tempfile
import shutil
import os
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

    def test_successful_open_paths_distinguishes_fd_returns_and_resolves_dirfds(self):
        trace = "\n".join((
            '10 open("/etc/ld.so.cache", O_RDONLY) = 3',
            '10 openat(AT_FDCWD, "/Lib/test", O_RDONLY|O_DIRECTORY) = 4',
            '10 openat2(4, "__pycache__/test_leaf.pyc", {flags=O_RDONLY}, 24) = 5',
            '10 openat(AT_FDCWD, "/Lib/test/missing.py", O_RDONLY) = -1 ENOENT (No such file or directory)',
        ))
        self.assertEqual(list(scorecard.successful_open_paths(trace, "/work")), [Path("/etc/ld.so.cache"), Path("/Lib/test"), Path("/Lib/test/__pycache__/test_leaf.pyc")])

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
            row = self.receipt(); directory = self.root / "stdlib"; directory.mkdir(); receipt = directory / "errno.jsonl"; receipt.write_text(json.dumps(row) + "\n")
            locator = self.locator(receipt); paths = [receipt, self.root / "O0.stderr", self.root / "O3.stderr"]; files = {self.locator(path):hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}
            (self.root / "activation.json").write_text(json.dumps({"schema":"imre.compatibility.snapshot-activation.v1", "imre_revision":PIN, "families":{"errno":{"adapter":"row-v1", "receipt":locator, "files":files}}}))
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

    def test_row_v1_booleans_cannot_earn_credit(self):
        r = self.receipt(); self.validate(r); row = {"originals": 1}; scorecard.summarize_family(row, [r]); self.assertEqual((row["observed"], row["paired"]), (1, 0))
        r["launcher_control"] = self.control(r); r["paired"] = True
        with patch.object(scorecard, "activation_entry", return_value={"adapter":"row-v1"}):
            with self.assertRaisesRegex(ValueError, "row-v1 credited pairs are unsupported"): self.validate(r)
        r["paired"] = False; self.validate(r)
        r["launcher_control"] = self.control(r, passed=False); self.validate(r)
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


@unittest.skipUnless(os.environ.get("SCORECARD_SHARD_NEGATIVES") == "1", "stage a snapshot activation manifest to run published-shard checks")
class PublishedShardTests(unittest.TestCase):
    """Read-only integration checks against activated published artifacts."""
    def setUp(self):
        self.addCleanup(patch.stopall)
        self.root = scorecard.HERE / "revisions" / PIN
        patch.object(scorecard, "DATA_ROOT", self.root).start()
        patch.object(scorecard, "COHORT", {**scorecard.CANONICAL_COHORT, "imre_revision": PIN}).start()
        self.activation = copy.deepcopy(scorecard.snapshot_activation())
        self.clear()

    def clear(self):
        for fn in (scorecard.adapt_family_replay, scorecard.adapt_pathlib, scorecard.adapt_urllib): fn.cache_clear()

    def row(self, family):
        return json.loads((self.root / "stdlib" / f"{family}.jsonl").read_text().splitlines()[0])

    def validate(self, row):
        scorecard.validate_record(row, "stdlib", scorecard.families())
        scorecard.verify_stdlib_evidence(row)

    def test_scoped_genuine_counts(self):
        fs, _ = scorecard.summary(); expected = {"configparser":(344,339), "fnmatch":(24,24), "pathlib":(6,6), "urllib":(178,174)}
        self.assertEqual({name:(fs[name]["receipts"],fs[name]["paired"]) for name in expected}, expected)

    def test_missing_or_altered_gate(self):
        gate = self.activation["families"]["pathlib"]["sources"]["gate"]
        altered = copy.deepcopy(self.activation); altered["families"]["pathlib"]["files"][gate] = "0" * 64
        with patch.object(scorecard, "snapshot_activation", return_value=altered):
            with self.assertRaisesRegex(ValueError, "digest differs"): self.validate(self.row("pathlib"))
        self.clear(); missing = copy.deepcopy(self.activation); entry = missing["families"]["pathlib"]; entry["sources"]["gate"] = gate + ".missing"; entry["files"][gate + ".missing"] = "0" * 64
        with patch.object(scorecard, "snapshot_activation", return_value=missing):
            with self.assertRaisesRegex(ValueError, "missing"): self.validate(self.row("pathlib"))

    def test_pathlib_requires_exact_positive_source_and_poison_hash(self):
        altered = copy.deepcopy(self.activation); del altered["families"]["pathlib"]["sources"]["positive_launcher"]
        with patch.object(scorecard, "snapshot_activation", return_value=altered):
            with self.assertRaisesRegex(ValueError, "missing bound positive_launcher"): self.validate(self.row("pathlib"))
        self.clear(); original = scorecard.artifact_json
        def changed(path):
            obj = original(path)
            if path.name == "gate.json":
                obj = copy.deepcopy(obj); obj["control"]["launcher_negative_sha256"] = "1" * 64
            return obj
        with patch.object(scorecard, "artifact_json", side_effect=changed):
            with self.assertRaisesRegex(ValueError, "poisoned launcher or import binding"): self.validate(self.row("pathlib"))

    def isolated_pathlib_trace(self, temporary, label, injected):
        """Copy only the activated pathlib evidence into a disposable tree."""
        root = Path(temporary); shutil.copytree(self.root / "evidence/pathlib", root / "evidence/pathlib")
        (root / "stdlib").mkdir(); old_prefix = self.root.relative_to(scorecard.ROOT).as_posix(); new_prefix = root.relative_to(scorecard.ROOT).as_posix()
        def remap(locator):
            self.assertTrue(locator.startswith(old_prefix + "/")); return new_prefix + locator[len(old_prefix):]
        entry = copy.deepcopy(self.activation["families"]["pathlib"])
        entry["receipt"] = remap(entry["receipt"]); entry["sources"] = {k:remap(v) for k,v in entry["sources"].items()}
        rows = [json.loads(line) for line in (self.root / "stdlib/pathlib.jsonl").read_text().splitlines()]
        for row in rows:
            for run in row["runs"].values():
                file, line = run["evidence"].split("#L"); run["evidence"] = remap(file) + "#L" + line
        (root / "stdlib/pathlib.jsonl").write_text("".join(json.dumps(row) + "\n" for row in rows))
        run_path = root / "evidence/pathlib/run.json"; manifest = json.loads(run_path.read_text()); manifest["gate"] = remap(manifest["gate"])
        for tier in ("O0", "O3"):
            for phase in ("baseline", "withdrawn"):
                obs = manifest["tiers"][tier][phase]; obs["trace"] = remap(obs["trace"])
        trace = root / "evidence/pathlib" / f"O0.{label}.trace"; trace.write_bytes(trace.read_bytes() + injected.encode())
        manifest["tiers"]["O0"][label]["trace_sha256"] = hashlib.sha256(trace.read_bytes()).hexdigest()
        run_path.write_text(json.dumps(manifest) + "\n")
        entry["files"] = {remap(k):hashlib.sha256((scorecard.ROOT / remap(k)).read_bytes()).hexdigest() for k in entry["files"]}
        (root / "activation.json").write_text(json.dumps({"schema":"imre.compatibility.snapshot-activation.v1", "imre_revision":PIN, "families":{"pathlib":entry}}))
        return root

    def test_pathlib_trace_mutation_rejects_successful_source_and_bytecode_opens(self):
        gate = json.loads((self.root / "evidence/pathlib/gate.json").read_text()); leaf = Path(gate["control"]["launcher"]); tree = leaf.parents[1]
        cases = (("baseline", f'9900 open("{leaf}", O_RDONLY) = 3\n', False),
                 ("withdrawn", f'9900 openat(AT_FDCWD, "{leaf.parent}/__pycache__/{leaf.stem}.cpython-314.pyc", O_RDONLY) = 3\n', False),
                 ("baseline", '9900 openat(AT_FDCWD, "test_join_posix.py", O_RDONLY) = 4\n', False),
                 ("baseline", '9900 openat(AT_FDCWD, "/other/cpython/Lib/test/test_pathlib/test_join_posix.py", O_RDONLY) = 4\n', False),
                 ("withdrawn", f'9900 openat2(AT_FDCWD, "{tree}/support/__init__.py", {{flags=O_RDONLY}}, 24) = 4\n', False),
                 ("withdrawn", f'9900 openat(AT_FDCWD, "{leaf}", O_RDONLY) = -1 ENOENT (No such file or directory)\n9900 open("/etc/localtime", O_RDONLY) = 3\n', True))
        for label, injected, accepted in cases:
            with self.subTest(label=label, injected=injected), tempfile.TemporaryDirectory(prefix=".scorecard-pathlib-trace-", dir=scorecard.ROOT) as temporary:
                root = self.isolated_pathlib_trace(temporary, label, injected); self.clear()
                with patch.object(scorecard, "DATA_ROOT", root):
                    if accepted:
                        fs, _ = scorecard.summary(); self.assertEqual(fs["pathlib"]["paired"], 6)
                    else:
                        with self.assertRaisesRegex(ValueError, "successfully opened test source, bytecode or withdrawn test tree"): scorecard.summary()

    def test_wrong_runtime(self):
        original = scorecard.artifact_json
        def changed(path):
            obj = original(path)
            if path.name == "run.json" and path.parent.name == "gate":
                obj = copy.deepcopy(obj); argv = obj["argv"]; argv[argv.index("--runtime")+1] = "old-runtime.a"
            return obj
        with patch.object(scorecard, "artifact_json", side_effect=changed):
            with self.assertRaisesRegex(ValueError, "identities missing|runtime"): self.validate(self.row("urllib"))

    def test_old_native_evidence(self):
        row = self.row("configparser"); row["runs"]["O0"]["evidence"] = scorecard.canonical_original(row, "stdlib")["runs"]["O0"]["evidence"]
        with self.assertRaisesRegex(ValueError, "outside allowed root"): self.validate(row)

    def test_missing_withdrawal_raw_proof(self):
        altered = copy.deepcopy(self.activation); entry = altered["families"]["pathlib"]; raw = next(path for path in entry["files"] if path.endswith("O0.withdrawn.stderr")); del entry["files"][raw]
        with patch.object(scorecard, "snapshot_activation", return_value=altered):
            with self.assertRaisesRegex(ValueError, "not digest-bound"): self.validate(self.row("pathlib"))

    def test_unbound_staged_family_is_pending(self):
        altered = copy.deepcopy(self.activation); del altered["families"]["fnmatch"]
        with patch.object(scorecard, "snapshot_activation", return_value=altered):
            fs, _ = scorecard.summary(); self.assertEqual((fs["fnmatch"]["pending"],fs["fnmatch"]["receipts"],fs["fnmatch"]["paired"]),(24,0,0))


if __name__ == "__main__": unittest.main()
