"""Rehashed real-trace attacks and immutable source-stage authority controls."""
import copy
import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import raw_control
import scorecard
from test_raw_control_staged import PIN, SNAPSHOT, evidence

def digest(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def loc(path): return path.relative_to(scorecard.ROOT).as_posix()
def dump(path, obj): path.parent.mkdir(parents=True, exist_ok=True); path.write_text(json.dumps(obj, indent=2)+"\n")


class RehashedTraceTests(unittest.TestCase):
    def test_real_binding_rejects_rehashed_payload_opens(self):
        directory, cohort, run = evidence("glob")
        original = json.loads((directory/"binding.json").read_text())
        raw = (directory/"O0.trace").read_text().splitlines()
        target = raw_control.execs(raw, scorecard.require)[1]
        pid = target[1]
        attacks = {
            "open": [f'{pid} open("/tmp/unapproved.py", O_RDONLY) = 40'],
            "openat": [f'{pid} openat(AT_FDCWD, "/tmp/unapproved.pyc", O_RDONLY) = 40'],
            "pyo": [f'{pid} openat(AT_FDCWD, "/tmp/unapproved.pyo", O_RDONLY) = 40'],
            "openat2": [f'{pid} openat2(AT_FDCWD, "/tmp/unapproved.zip", {{flags=O_RDONLY}}, 24) = 40'],
            "creat": [f'{pid} creat("/tmp/no-suffix", 0600) = 40'],
            "dirfd-traversal": [f'{pid} openat(AT_FDCWD, "/tmp/sub", O_RDONLY|O_DIRECTORY) = 40',
                                f'{pid} openat(40, "../unapproved.whl", O_RDONLY) = 41'],
            "proc-fd": [f'{pid} openat(AT_FDCWD, "/proc/self/fd/7", O_RDONLY) = 40'],
            "dev-fd": [f'{pid} openat(AT_FDCWD, "/dev/fd/8", O_RDONLY) = 40'],
            "fd-traversal": [f'{pid} openat(AT_FDCWD, "/proc/self/fd", O_RDONLY|O_DIRECTORY) = 40',
                             f'{pid} openat(40, "7/../unapproved", O_RDONLY) = 41'],
            "relative-fd": [f'{pid} openat(AT_FDCWD, "/proc/thread-self", O_RDONLY|O_DIRECTORY) = 40',
                            f'{pid} openat2(40, "fd/9", {{flags=O_RDONLY}}, 24) = 41'],
        }
        with tempfile.TemporaryDirectory(dir=SNAPSHOT) as tmp:
            tmp=Path(tmp)
            receipt=tmp/"receipt.jsonl"
            receipt.write_text("".join(json.dumps({**x,"paired":True})+"\n" for x in original["originals"]))
            for name, calls in attacks.items():
                with self.subTest(name=name):
                    trace=tmp/(name+".trace"); trace.write_text("\n".join(raw[:target[0]+1]+calls+raw[target[0]+1:])+"\n")
                    binding=copy.deepcopy(original);binding["cohorts"]["finite-native"]["artifacts"]["O0_trace"]=loc(trace)
                    b=tmp/(name+".json");dump(b,binding)
                    files={p:digest(scorecard.ROOT/p) for p in set(binding["cohorts"]["finite-native"]["artifacts"].values())}
                    files.update({loc(b):digest(b),loc(receipt):digest(receipt)})
                    entry={"receipt":loc(receipt),"sources":{"binding":loc(b)},"files":files}
                    with patch.object(scorecard,"DATA_ROOT",SNAPSHOT), patch.object(scorecard,"COHORT",{**scorecard.CANONICAL_COHORT,"imre_revision":PIN}), patch.object(scorecard,"activation_entry",return_value=entry):
                        with self.assertRaisesRegex(ValueError,"payload|creat|unauthorized"):
                            raw_control.adapt(scorecard,"glob",loc(b),digest(b),json.dumps(run["source"],sort_keys=True),name,"finite-native")

    def test_legacy_scope_does_not_reject_unrelated_source_open(self):
        directory, binding, run=evidence("logging");lines=(directory/"O0.trace").read_text().splitlines()
        target=raw_control.execs(lines,scorecard.require)[1];lines.insert(target[0]+1,f'{target[1]} openat(AT_FDCWD, "/tmp/unapproved.py", O_RDONLY) = 40')
        tree=str(Path(run["runs"]["O0"]["environment"]["PYTHONPATH"])/"test")
        raw_control.trace_withdraw(scorecard,"\n".join(lines),run["runs"]["O0"]["argv"][0],binding["probe"],tree,tree+".raw-control-stash",tree+"/test_logging.py",run["runs"]["O0"]["cwd"],scorecard.require,binding["withdraw_sources"])


class AuthorityTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(dir=scorecard.ROOT);self.addCleanup(self.temp.cleanup);self.root=Path(self.temp.name)
        self.git("init","-q");self.git("config","user.name","Charlie Marsh");self.git("config","user.email","charlie.r.marsh@gmail.com");self.git("remote","add","origin","https://github.com/astral-experiments/imre.git")
        self.data=self.root/"benchmarks/compatibility/revisions"/PIN;self.data.mkdir(parents=True)
        source=self.root/"adapter.py";source.write_text("adapter\n");control=self.data/"control.c";control.write_text("control\n")
        staged=self.data/"stdlib/glob.jsonl";staged.parent.mkdir();staged.write_text("stage\n")
        binding=self.data/"evidence/glob/binding.json";dump(binding,{"cohorts":{"finite":{"strict_source_control":"native-content-v1","probe":"fixed-probe","artifacts":{"probe_source":self.rel(control)}}}})
        self.binding=binding;self.control=control
        family={"strict_source_control":"native-content-v1","binding":{"path":self.rel(binding),"sha256":digest(binding)},"staged_receipt":{"path":self.rel(staged),"sha256":digest(staged),"originals":24,"frozen_originals":24},"evidence_files":{self.rel(binding):digest(binding),self.rel(control):digest(control)},"artifact_files":[self.rel(control)]}
        manifest={"schema":"imre.compatibility.source-stage.v1","imre_revision":PIN,"sources":{self.rel(source):digest(source),self.rel(control):digest(control)},"families":{"glob":family}}
        self.manifest=self.root/raw_control.SOURCE_MANIFEST;dump(self.manifest,manifest)
        self.git("add",".");self.git("commit","-qm","source stage");self.stage=self.git("rev-parse","HEAD").decode().strip();self.git("update-ref","refs/remotes/origin/main",self.stage)
        receipt=self.data/"verified/stdlib/glob.jsonl";receipt.parent.mkdir(parents=True);receipt.write_text("receipt\n");self.git("add",".");self.git("commit","-qm","activation")
        raw=self.manifest.read_bytes();anchor={"commit":self.stage,"manifest":raw_control.SOURCE_MANIFEST,"git_blob":hashlib.sha1(f"blob {len(raw)}\0".encode()+raw).hexdigest(),"sha256":digest(self.manifest)}
        fixed={**manifest["sources"],**family["evidence_files"],self.rel(staged):digest(staged),self.rel(binding):digest(binding)}
        self.entry={"adapter":"raw-control-v1","receipt":self.rel(receipt),"staged_receipt":self.rel(staged),"sources":{"binding":self.rel(binding)},"files":{**fixed,self.rel(receipt):digest(receipt)},"source_stage":anchor}
        self.addCleanup(patch.stopall);patch.object(scorecard,"ROOT",self.root).start();patch.object(scorecard,"DATA_ROOT",self.data).start();patch.object(scorecard,"COHORT",{**scorecard.CANONICAL_COHORT,"imre_revision":PIN}).start();patch.object(scorecard,"activation_entry",side_effect=lambda *args:self.entry).start();raw_control.source_stage_authority.cache_clear()

    def git(self,*args):return subprocess.check_output(["git",*args],cwd=self.root,stderr=subprocess.DEVNULL)
    def rel(self,path):return path.relative_to(self.root).as_posix()
    def validate(self):return raw_control.source_stage_authority(scorecard,"glob",hashlib.sha256(json.dumps(self.entry,sort_keys=True).encode()).hexdigest())

    def test_public_ancestor_and_git_blob(self):
        self.validate()
        self.manifest.write_text("untrusted local manifest\n")
        raw_control.source_stage_authority.cache_clear()
        self.validate()  # the authority loads the committed blob, not this file

    def test_self_unreachable_and_wrong_blob_rejected(self):
        for value in (self.git("rev-parse","HEAD").decode().strip(),"0"*40):
            with self.subTest(commit=value):
                self.entry["source_stage"]["commit"]=value
                with self.assertRaises(ValueError):self.validate()
        self.entry["source_stage"]["commit"]=self.stage;self.entry["source_stage"]["git_blob"]="0"*40
        with self.assertRaisesRegex(ValueError,"Git blob"):self.validate()

    def test_rehashed_strict_downgrade_or_probe_rebind_rejected(self):
        original=self.binding.read_bytes()
        for change in ("delete-mode","downgrade-mode","rebind-probe"):
            binding=json.loads(original);c=binding["cohorts"]["finite"]
            if change=="delete-mode":del c["strict_source_control"]
            elif change=="downgrade-mode":c["strict_source_control"]="direct-and-withdraw-v1"
            else:c["probe"]="different-probe"
            dump(self.binding,binding);self.entry["files"][self.rel(self.binding)]=digest(self.binding)
            # A mutable local manifest is not an alternate authority.
            dump(self.manifest,{"families":{"glob":{"binding":{"sha256":digest(self.binding)}}}})
            with self.subTest(change=change),self.assertRaisesRegex(ValueError,"immutable source stage"):self.validate()

    def test_rehashed_bound_adapter_and_probe_source_mutation_rejected(self):
        for path in (self.root/"adapter.py",self.control):
            previous=path.read_bytes();path.write_bytes(previous+b"changed\n")
            self.entry["files"][self.rel(path)]=digest(path)
            with self.subTest(path=path.name),self.assertRaisesRegex(ValueError,"immutable source stage"):
                self.validate()
            path.write_bytes(previous);self.entry["files"][self.rel(path)]=digest(path)

    def test_missing_authority_cannot_downgrade(self):
        del self.entry["source_stage"]
        with self.assertRaisesRegex(ValueError,"fixed source-stage"):self.validate()


if __name__=="__main__":unittest.main()
