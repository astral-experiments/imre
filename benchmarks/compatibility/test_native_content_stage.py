"""Offline, rehashed attacks on the selected run-only glob/logging bindings."""
import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import raw_control
import scorecard
from test_raw_control_staged import PIN, SNAPSHOT

RUN_ONLY = SNAPSHOT / "evidence/run-only"

def digest(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def loc(path): return path.relative_to(scorecard.ROOT).as_posix()
def dump(path, obj): path.write_text(json.dumps(obj, indent=2)+"\n")


class NativeContentStageTests(unittest.TestCase):
    def validate(self, family, binding_path, receipt, nonce):
        binding=json.loads(binding_path.read_text()); cohort=binding["cohorts"]["finite-native"]; archive=json.loads((RUN_ONLY/"archive.json").read_text())
        paths=set(cohort["artifacts"].values()) | set(archive["families"][family]) | {loc(binding_path),loc(receipt)}
        entry={"receipt":loc(receipt),"sources":{"binding":loc(binding_path)},"files":{p:digest(scorecard.ROOT/p) for p in paths}}
        run=json.loads((scorecard.ROOT/cohort["artifacts"]["run"]).read_text())
        with patch.object(scorecard,"DATA_ROOT",SNAPSHOT),patch.object(scorecard,"COHORT",{**scorecard.CANONICAL_COHORT,"imre_revision":PIN}),patch.object(scorecard,"activation_entry",return_value=entry):
            return raw_control.adapt(scorecard,family,loc(binding_path),digest(binding_path),json.dumps(run["source"],sort_keys=True),nonce,"finite-native")

    def receipt(self, folder, binding):
        path=folder/"receipt.jsonl";path.write_text("".join(json.dumps({**x,"paired":True})+"\n" for x in binding["originals"]));return path

    def test_all_four_selected_traces_and_outputs(self):
        with tempfile.TemporaryDirectory(dir=SNAPSHOT) as folder:
            for family in ("glob","logging"):
                path=RUN_ONLY/family/"binding.json";binding=json.loads(path.read_text())
                run,events,_=self.validate(family,path,self.receipt(Path(folder),binding),"positive-"+family)
                self.assertEqual(list(events["stock"]),list(events["O0"]))
                self.assertEqual(list(events["O0"]),list(events["O3"]))
                self.assertEqual(len(events["stock"]),21 if family=="glob" else 1)

    def test_rehashed_unapproved_content_and_open_variants(self):
        path=RUN_ONLY/"glob/binding.json";original=json.loads(path.read_text());cohort=original["cohorts"]["finite-native"];a=cohort["artifacts"]
        lines=(scorecard.ROOT/a["O0_trace"]).read_text().splitlines();target=raw_control.execs(lines,scorecard.require)[1];pid=target[1]
        cwd=json.loads((scorecard.ROOT/a["run"]).read_text())["runs"]["O0"]["cwd"];fixture=f"{cwd}/@test_{pid}_tmp\\303\\246_dir"
        at=f"AT_FDCWD<{cwd}>";ro="O_RDONLY|O_CLOEXEC"
        attacks={
            "extensionless":f'{pid} openat({at}, "/tmp/unrelated", {ro}) = 40</tmp/unrelated>',
            "renamed-python":f'{pid} open("/tmp/renamed.py", {ro}) = 40</tmp/renamed.py>',
            "pyz":f'{pid} openat({at}, "/tmp/payload.pyz", {ro}) = 40</tmp/payload.pyz>',
            "egg":f'{pid} openat({at}, "/tmp/payload.egg", {ro}) = 40</tmp/payload.egg>',
            "dirfd-relative":f'{pid} openat(40</tmp>, "unrelated", {ro}) = 41</tmp/unrelated>',
            "dirfd-traversal":f'{pid} openat(40</tmp/sub>, "../unrelated", {ro}) = 41</tmp/unrelated>',
            "unknown-dirfd":f'{pid} openat(40, "unrelated", {ro}) = 41</tmp/unrelated>',
            "proc-task-alias":f'{pid} openat({at}, "/proc/self/task/{pid}/fd/7", {ro}) = 41</tmp/unrelated>',
            "write-mode":f'{pid} openat({at}, "{fixture}/a/D", O_RDWR|O_CREAT, 0600) = 41<{fixture}/a/D>',
            "unknown-fixture":f'{pid} openat({at}, "{fixture}/unknown", O_WRONLY|O_CREAT|O_TRUNC|O_CLOEXEC, 0777) = 41<{fixture}/unknown>',
            "openat2":f'{pid} openat2({at}, "/tmp/unrelated", {{flags={ro}, resolve=0}}, 24) = 41</tmp/unrelated>',
            "creat":f'{pid} creat("/tmp/unrelated", 0600) = 41</tmp/unrelated>',
        }
        with tempfile.TemporaryDirectory(dir=SNAPSHOT) as folder:
            folder=Path(folder);receipt=self.receipt(folder,original)
            for name,call in attacks.items():
                with self.subTest(attack=name):
                    trace=folder/(name+".trace");trace.write_text("\n".join(lines[:target[0]+1]+[call]+lines[target[0]+1:])+"\n")
                    rec=json.loads((scorecard.ROOT/a["O0_recapture"]).read_text());rec["trace_sha256"]=digest(trace);rec["argv"][9]=str(trace)
                    rp=folder/(name+".run.json");dump(rp,rec);binding=copy.deepcopy(original);roles=binding["cohorts"]["finite-native"]["artifacts"]
                    roles["O0_trace"]=loc(trace);roles["O0_recapture"]=loc(rp);bp=folder/(name+".binding.json");dump(bp,binding)
                    with self.assertRaisesRegex(ValueError,"unauthorized|unknown kernel dirfd"):
                        self.validate("glob",bp,receipt,name)

    def test_uncredited_manifest_matches_selected_bytes(self):
        manifest=json.loads((scorecard.ROOT/raw_control.SOURCE_MANIFEST).read_text())
        for path,value in manifest["sources"].items():
            self.assertEqual(digest(scorecard.ROOT/path),value,path)
        for family in ("glob","logging"):
            frozen=manifest["families"][family];self.assertEqual(frozen["strict_source_control"],"native-content-v1")
            binding=json.loads((scorecard.ROOT/frozen["binding"]["path"]).read_text())
            self.assertEqual(frozen["binding"]["sha256"],digest(scorecard.ROOT/frozen["binding"]["path"]))
            self.assertEqual(frozen["staged_receipt"]["sha256"],digest(scorecard.ROOT/frozen["staged_receipt"]["path"]))
            artifacts={v for c in binding["cohorts"].values() for v in c["artifacts"].values()}
            self.assertEqual(set(frozen["artifact_files"]),artifacts)
            fixed={**manifest["sources"],**frozen["evidence_files"]}
            self.assertTrue(artifacts <= set(fixed))
            for path,value in frozen["evidence_files"].items():
                self.assertEqual(digest(scorecard.ROOT/path),value,path)

    def test_changed_source_line_witness_rejected(self):
        policy=json.loads((RUN_ONLY/"policy.json").read_text());policy["sources"]["glob"]["witnesses"][0]["sha256"]="0"*64
        with patch.object(scorecard,"DATA_ROOT",SNAPSHOT),patch.object(scorecard,"COHORT",{**scorecard.CANONICAL_COHORT,"imre_revision":PIN}):
            with self.assertRaisesRegex(ValueError,"line witness"):
                raw_control.policy_sources(scorecard,policy,scorecard.require)


if __name__=="__main__":unittest.main()
