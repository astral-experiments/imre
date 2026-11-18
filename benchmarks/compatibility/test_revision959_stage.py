"""Offline, rebound controls for the inactive revision959 stage."""
import copy
import hashlib
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import raw_control
import scorecard
import script_control

PIN=script_control.PIN
SNAPSHOT=scorecard.HERE/'revisions'/PIN

def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def dump(path,value): path.write_text(json.dumps(value,indent=2)+'\n')

class FiniteStageTests(unittest.TestCase):
    def setUp(self):
        self.t=tempfile.TemporaryDirectory(dir=scorecard.ROOT);self.addCleanup(self.t.cleanup)
        self.data=Path(self.t.name)/'snapshot';shutil.copytree(SNAPSHOT,self.data)
        self.directory=self.data/'evidence/fnmatch';self.binding_path=self.directory/'binding.json';self.binding=json.loads(self.binding_path.read_text());old=SNAPSHOT.relative_to(scorecard.ROOT).as_posix();new=self.data.relative_to(scorecard.ROOT).as_posix()
        for key,value in self.cohort()['artifacts'].items():self.cohort()['artifacts'][key]=value.replace(old,new,1)
        dump(self.binding_path,self.binding);self.entry={'adapter':'raw-control-v1','receipt':self.loc(self.data/'stdlib/fnmatch.jsonl'),'sources':{'binding':self.loc(self.binding_path)},'files':{}}
        for path in [self.binding_path,self.data/'stdlib/fnmatch.jsonl',*[scorecard.ROOT/x for x in self.cohort()['artifacts'].values()]]:self.rehash(path)
        self.addCleanup(patch.stopall);patch.object(scorecard,'DATA_ROOT',self.data).start();patch.object(scorecard,'COHORT',{**scorecard.CANONICAL_COHORT,'imre_revision':PIN}).start();patch.object(scorecard,'activation_entry',return_value=self.entry).start();raw_control.adapt.cache_clear();raw_control.load_binding.cache_clear()
    def cohort(self):return self.binding['cohorts']['finite-959']
    def loc(self,p):return p.relative_to(scorecard.ROOT).as_posix()
    def rehash(self,p):self.entry['files'][self.loc(p)]=sha(p)
    def artifact(self,role):return scorecard.ROOT/self.cohort()['artifacts'][role]
    def change(self,role,mutate):
        p=self.artifact(role);obj=json.loads(p.read_text());mutate(obj);dump(p,obj);self.rehash(p)
    def adapt(self):
        row=json.loads((self.data/'stdlib/fnmatch.jsonl').read_text().splitlines()[0]);return raw_control.adapt(scorecard,'fnmatch',self.loc(self.binding_path),sha(self.binding_path),json.dumps(row['source'],sort_keys=True),hashlib.sha256(json.dumps(self.entry,sort_keys=True).encode()).hexdigest(),'finite-959')
    def test_authentic_finite_capture(self):
        _,inventories,_=self.adapt();self.assertEqual({t:len(x) for t,x in inventories.items()},{'stock':24,'O0':24,'O3':24})
    def test_rehashed_stock_flag_and_source_downgrade(self):
        self.change('run',lambda x:x['runs']['stock']['environment'].__setitem__('IMREFLAGS',''))
        with self.assertRaisesRegex(ValueError,'stock finite configuration'):self.adapt()
    def test_rehashed_strict_mode_and_source_mutation(self):
        self.cohort()['strict_source_control']='direct-and-withdraw-v1';dump(self.binding_path,self.binding);self.rehash(self.binding_path)
        with self.assertRaisesRegex(ValueError,'strict mode'):self.adapt()
        self.cohort()['strict_source_control']='native-content-v1';dump(self.binding_path,self.binding);self.rehash(self.binding_path);p=self.artifact('stock_source');p.write_bytes(p.read_bytes()+b'\n');self.rehash(p)
        with self.assertRaisesRegex(ValueError,'pinned source'):self.adapt()
    def test_rehashed_cross_tuple(self):
        self.change('O0_build_json',lambda x:x.__setitem__('compiler_sha256','c8'+'0'*62))
        with self.assertRaisesRegex(ValueError,'custody differs'):self.adapt()
    def test_rehashed_effect_change(self):
        p=self.artifact('O0_effects_after');p.write_text('["changed"]\n');self.rehash(p)
        self.change('run',lambda x:x['runs']['O0'].__setitem__('effects_after_sha256',sha(p)))
        with self.assertRaisesRegex(ValueError,'effects differ'):self.adapt()
    def test_rehashed_unapproved_open(self):
        p=self.artifact('O0_trace');lines=p.read_text().splitlines();target=raw_control.execs(lines,scorecard.require)[1];pid=target[1];lines.insert(target[0]+1,f'{pid} openat(AT_FDCWD<{self.data}>, "/tmp/unapproved", O_RDONLY|O_CLOEXEC) = 40</tmp/unapproved>');p.write_text('\n'.join(lines)+'\n');self.rehash(p);digest=sha(p)
        self.change('run',lambda x:x['runs']['O0'].__setitem__('trace_sha256',digest));self.change('O0_meta',lambda x:x.__setitem__('trace_sha256',digest))
        with self.assertRaisesRegex(ValueError,'unauthorized'):self.adapt()
    def test_zero_credit_guard_before_mutable_adapter(self):
        row=json.loads((self.data/'stdlib/fnmatch.jsonl').read_text().splitlines()[0]);row['paired']=True
        with self.assertRaisesRegex(ValueError,'separately pinned public source stage'):scorecard.validate_snapshot_record(row,'stdlib',True,scorecard.require)
    def test_canonical_84_not_run_and_seven_anchor_hashes(self):
        rows=[json.loads(l) for f in (self.data/'scripts').glob('*.jsonl') for l in f.read_text().splitlines()];self.assertEqual({x['id'] for x in rows},set(scorecard.canonical_script_shards()));self.assertEqual(sum(all(v['outcome']=='pass' for v in x['runs'].values()) for x in rows),4);self.assertEqual(sum(all(v['outcome']=='not_run' for v in x['runs'].values()) for x in rows),80);self.assertTrue(all(not x['paired'] for x in rows));auth=json.loads((self.data/'controls/original-authority.json').read_text());self.assertEqual(len(auth['reviewed_frozen_anchors']),7);self.assertEqual(auth['commit'],script_control.ORIGINAL_AUTHORITY)

class TyTraceTests(unittest.TestCase):
    def test_exact_chain_and_rehashed_mask_open_exec_negatives(self):
        p=SNAPSHOT/'evidence/scripts/ty';load=lambda f:json.loads((p/f).read_text());report=load('original/report.json');smoke=json.loads((SNAPSHOT/'evidence/scripts/env-parity/ty/mask-smoke.json').read_text());st=(SNAPSHOT/'evidence/scripts/env-parity/ty/mask-smoke.trace').read_text();policy=json.loads((SNAPSHOT/'evidence/scripts/native-open-policy.json').read_text())
        for tier in ('O0','O3'):
            base=SNAPSHOT/'evidence/scripts/env-parity/ty/ty-05'/tier;meta=json.loads((base/'result.json').read_text());trace=(base/'run.trace').read_text();build=report['builds'][tier];script_control.script_withdrawal(scorecard,'ty-05',meta,trace,smoke,st,build,policy)
            if tier!='O0':continue
            bad=copy.deepcopy(meta);bad['bwrap_sha256']='0'*64
            with self.assertRaisesRegex(ValueError,'wrapper'):script_control.script_withdrawal(scorecard,'ty-05',bad,trace,smoke,st,build,policy)
            altered=trace.replace('--tmpfs", "'+str(Path(build['argv'][2]).parent),'--tmpfs", "/tmp/wrong',1);bad=copy.deepcopy(meta);bad['trace_sha256']=hashlib.sha256(altered.encode()).hexdigest()
            with self.assertRaisesRegex(ValueError,'wrapper|supervisor'):script_control.script_withdrawal(scorecard,'ty-05',bad,altered,smoke,st,build,policy)
            lines=trace.splitlines();target=raw_control.exec_sequence(lines,scorecard.require,4)[-1];pid=target[1]
            for call in (f'{pid} openat(AT_FDCWD<{meta["cwd"]}>, "/tmp/no-extension", O_RDONLY|O_CLOEXEC) = 40</tmp/no-extension>',f'{pid} execve("/usr/bin/python3", ["/usr/bin/python3"], 0x0 /* 0 vars */) = 0'):
                modified='\n'.join(lines[:target[0]+1]+[call]+lines[target[0]+1:])+'\n';bad=copy.deepcopy(meta);bad['trace_sha256']=hashlib.sha256(modified.encode()).hexdigest()
                with self.assertRaisesRegex(ValueError,'unauthorized|unverified exec'):script_control.script_withdrawal(scorecard,'ty-05',bad,modified,smoke,st,build,policy)

class ScriptStageTests(unittest.TestCase):
    def test_source_manifest_exact_stage_and_zero_credit(self):
        import gzip
        manifest=SNAPSHOT/'controls/unified-source-stage.json';m=json.loads(manifest.read_text());staged={x['path']:x['sha256'] for group in m['staged'].values() for x in group.values()};files={**m['artifacts'],**staged}
        self.assertEqual(set(files),{p.relative_to(scorecard.ROOT).as_posix() for p in SNAPSHOT.rglob('*') if p.is_file() and p!=manifest})
        for loc,digest in {**m['sources'],**files}.items():self.assertEqual(sha(scorecard.ROOT/loc),digest,loc)
        for id,ref in m['bindings']['script'].items():
            binding=json.loads((scorecard.ROOT/ref['path']).read_text());self.assertEqual(ref['sha256'],sha(scorecard.ROOT/ref['path']));self.assertEqual(ref['strict_source_control'],binding['strict_source_control']);self.assertEqual(binding['id'],id)
        tuple=m['build_tuples']['ruff_a'];tool=json.loads((SNAPSHOT/'custody/ruff-a-native-toolchain.json').read_text());ref=tuple['retrospective_transcript'];self.assertEqual(tuple['cargo_build_argv'],tool['cargo_build_argv']);self.assertEqual(tuple['cargo_build_environment'],tool['cargo_build_environment']);self.assertEqual(ref['sha256'],sha(scorecard.ROOT/ref['path']));self.assertEqual(tuple['custody_files'][ref['path']],ref['sha256'])
        old=json.loads((SNAPSHOT/'evidence/scripts/uvb/export-manifest.json').read_text());fresh=json.loads((SNAPSHOT/'evidence/scripts/stock-before/export-manifest.json').read_text())
        for case,ref in m['compressed_stock_traces'].items():
            path=scorecard.ROOT/ref['path'];raw=gzip.decompress(path.read_bytes());self.assertEqual((sha(path),path.stat().st_size),(ref['compressed_sha256'],ref['compressed_bytes']));self.assertEqual((hashlib.sha256(raw).hexdigest(),len(raw)),(ref['raw_sha256'],ref['raw_bytes']));self.assertEqual(fresh[case+'/stock.trace'],{'sha256':ref['raw_sha256'],'bytes':len(raw)})
            prior=m['nonselected_prior_stock'][case];self.assertEqual(old[prior['exported_path']]['sha256'],prior['sha256']);self.assertFalse((SNAPSHOT/'evidence/scripts/uvb'/prior['exported_path']).exists())
        self.assertFalse((SNAPSHOT/'activation.json').exists());self.assertFalse((SNAPSHOT/'verified').exists())
    def test_rehashed_ruff_a_retrospective_transcript_is_not_authority(self):
        binding=json.loads((SNAPSHOT/'evidence/scripts/uv-10.binding.json').read_text());entry={'files':{loc:sha(scorecard.ROOT/loc) for loc in binding['artifacts'].values()}};original=raw_control.bound_artifacts(scorecard,entry,binding['artifacts']);evidence=json.loads(original('native_toolchain').read_text())
        script_control.ruff_a_transcript(scorecard,original,evidence)
        with tempfile.TemporaryDirectory(dir=scorecard.ROOT) as tmp:
            path=Path(tmp)/'rebound.json';doc=json.loads(original('custody_transcript').read_text());doc['original_evidence_sha256']['build.log']='0'*64;dump(path,doc);digest=sha(path);evidence['cargo_transcript'].update(path=path.relative_to(scorecard.ROOT).as_posix(),sha256=digest)
            rebound=lambda role:path if role=='custody_transcript' else original(role)
            with self.assertRaisesRegex(ValueError,'transcript digest'):script_control.ruff_a_transcript(scorecard,rebound,evidence)
            with patch.object(script_control,'RUFF_A_TRANSCRIPT_SHA256',digest),self.assertRaisesRegex(ValueError,'original evidence hashes'):script_control.ruff_a_transcript(scorecard,rebound,evidence)

    def test_public_anchor_rebound_negatives(self):
        index=json.loads((SNAPSHOT/'controls/original-authority.json').read_text());script_control.audit_original_authority(scorecard,index)
        for field,value in [('raw_line_sha256','0'*64),('canonical_file_commit','0'*40),('archived_original_argv',['changed']),('source',{'path':'incorrect'})]:
            bad=copy.deepcopy(index);bad['reviewed_frozen_anchors']['ty-05'][field]=value
            with self.subTest(field=field),self.assertRaisesRegex(ValueError,'reviewed frozen anchor'):script_control.audit_original_authority(scorecard,bad)
    def test_strict_candidates_fail_closed_on_actual_evidence(self):
        with patch.object(scorecard,'DATA_ROOT',SNAPSHOT),patch.object(scorecard,'COHORT',{**scorecard.CANONICAL_COHORT,'imre_revision':PIN}):
            for id,shard,reason in [('uv-10','uv-a',None),('uv-26','uv-b',None),('uv-29','uv-b',None),('ty-05','ty',None)]:
                row=next(json.loads(x) for x in (SNAPSHOT/'scripts'/(shard+'.jsonl')).read_text().splitlines() if json.loads(x)['id']==id);binding=json.loads((SNAPSHOT/'evidence/scripts'/(id+'.binding.json')).read_text());entry={'files':{loc:sha(scorecard.ROOT/loc) for loc in binding['artifacts'].values()}}
                if reason:
                    with self.subTest(id=id),self.assertRaisesRegex(ValueError,reason):script_control.strict_case(scorecard,row,binding,entry)
                else:script_control.strict_case(scorecard,row,binding,entry)
                row['paired']=True
                with self.assertRaisesRegex(ValueError,'separately pinned public source stage'):scorecard.validate_snapshot_record(row,'script',True,scorecard.require)
    def test_existing_scorecard_derives_frozen_eligibility(self):
        anchors=scorecard.load_frozen_scripts();observed={}
        with patch.object(scorecard,'DATA_ROOT',SNAPSHOT),patch.object(scorecard,'COHORT',{**scorecard.CANONICAL_COHORT,'imre_revision':PIN}),patch.object(scorecard,'validate_snapshot_record',side_effect=lambda r,k,eligible,c:observed.__setitem__(r['id'],eligible)):
            for shard in ('uv-a','uv-b','ty'):
                for line in (SNAPSHOT/'scripts'/(shard+'.jsonl')).read_text().splitlines():
                    row=json.loads(line)
                    if row['id'] in {'uv-10','uv-26','uv-29','ty-05'}:scorecard.validate_record(row,'script',{},anchors)
        self.assertEqual(observed,{'uv-10':True,'uv-26':True,'uv-29':True,'ty-05':True})
    def test_launch_configuration_and_case_tuple_reject_mutations(self):
        row=next(json.loads(x) for x in (SNAPSHOT/'scripts/ty.jsonl').read_text().splitlines() if json.loads(x)['id']=='ty-05');bad=copy.deepcopy(row);bad['runs']['O0']['environment']['HOME']='/tmp/unapproved'
        with self.assertRaisesRegex(ValueError,'home role'):script_control.launch_config(scorecard,bad)
        with patch.object(scorecard,'DATA_ROOT',SNAPSHOT),patch.object(scorecard,'COHORT',{**scorecard.CANONICAL_COHORT,'imre_revision':PIN}):
            binding=json.loads((SNAPSHOT/'evidence/scripts/ty-05.binding.json').read_text());entry={'files':{loc:sha(scorecard.ROOT/loc) for loc in binding['artifacts'].values()}};bad=copy.deepcopy(row);bad['runs']['O3']['toolchain']['compiler_sha256']='c8'+'0'*62
            with self.assertRaisesRegex(ValueError,'build tuple'):script_control.strict_case(scorecard,bad,binding,entry)

class PublicAuthorityTests(unittest.TestCase):
    def test_rebound_source_and_strict_binding_cannot_change_file_set(self):
        with tempfile.TemporaryDirectory(dir=scorecard.ROOT) as tmp:
            root=Path(tmp);data=root/'benchmarks/compatibility/revisions'/PIN;source=root/'adapter.py';source.write_text('adapter\n');binding=data/'evidence/scripts/ty-05.binding.json';binding.parent.mkdir(parents=True);dump(binding,{'strict_source_control':'ty05-exact-bwrap-v1'});staged=data/'scripts/ty.jsonl';staged.parent.mkdir();staged.write_text('stage\n');verified=data/'verified/scripts/ty.jsonl';verified.parent.mkdir(parents=True);verified.write_text('receipt\n');loc=lambda p:p.relative_to(root).as_posix();ref={'path':loc(binding),'sha256':sha(binding),'shard':'ty'};manifest={'sources':{loc(source):sha(source)},'artifacts':{loc(binding):sha(binding)},'staged':{'script':{'ty':{'path':loc(staged),'sha256':sha(staged)}},'stdlib':{}},'bindings':{'script':{'ty-05':ref},'stdlib':{}}};fixed={**manifest['sources'],**manifest['artifacts'],loc(staged):sha(staged)};entry={'adapter':'script-control-v1','receipt':loc(verified),'staged_receipt':loc(staged),'sources':{'ty-05':loc(binding)},'files':{**fixed,loc(verified):sha(verified)},'source_stage':{}};row={'kind':'script','id':'ty-05'}
            with patch.object(scorecard,'ROOT',root),patch.object(scorecard,'DATA_ROOT',data):
                script_control.validate_file_set(scorecard,row,entry,manifest)
                for path in (source,binding):
                    old=path.read_bytes();path.write_bytes(old+b'changed');entry['files'][loc(path)]=sha(path)
                    with self.assertRaisesRegex(ValueError,'immutable 959 stage'):script_control.validate_file_set(scorecard,row,entry,manifest)
                    path.write_bytes(old);entry['files'][loc(path)]=sha(path)
    def test_fixed_public_ancestor_and_rebound_tuple(self):
        with tempfile.TemporaryDirectory(dir=scorecard.ROOT) as tmp:
            root=Path(tmp)
            def git(*args):return subprocess.check_output(['git',*args],cwd=root,stderr=subprocess.DEVNULL).decode().strip()
            git('init','-q');git('config','user.name','Charlie Marsh');git('config','user.email','charlie.r.marsh@gmail.com');git('remote','add','origin','https://github.com/astral-experiments/imre.git');p=root/script_control.MANIFEST;p.parent.mkdir(parents=True);dump(p,{'schema':'imre.compatibility.source-stage.v1','imre_revision':PIN,'original_authority':{'commit':script_control.ORIGINAL_AUTHORITY}});git('add','.');git('commit','-qm','stage');stage=git('rev-parse','HEAD');blob=git('rev-parse',f'{stage}:{script_control.MANIFEST}');git('update-ref','refs/remotes/origin/main',stage);(root/'later').write_text('later');git('add','.');git('commit','-qm','activation');fixed={'commit':stage,'manifest':script_control.MANIFEST,'git_blob':blob,'sha256':sha(p)};entry={'source_stage':copy.deepcopy(fixed)}
            with patch.object(scorecard,'ROOT',root),patch.object(script_control.source_stage_pins,'REVISION_959',fixed):
                script_control.authority(scorecard,entry);entry['source_stage']['git_blob']='0'*40
                with self.assertRaisesRegex(ValueError,'fixed public'):script_control.authority(scorecard,entry)
                entry['source_stage']=copy.deepcopy(fixed);entry['source_stage']['commit']=git('rev-parse','HEAD')
                with self.assertRaises(ValueError):script_control.authority(scorecard,entry)


class ScriptReboundTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(dir=scorecard.ROOT);self.addCleanup(self.temp.cleanup)
        self.data=Path(self.temp.name)/'snapshot';shutil.copytree(SNAPSHOT,self.data)
        self.addCleanup(patch.stopall);patch.object(scorecard,'DATA_ROOT',self.data).start();patch.object(scorecard,'COHORT',{**scorecard.CANONICAL_COHORT,'imre_revision':PIN}).start()
        self.row=next(json.loads(x) for x in (self.data/'scripts/uv-b.jsonl').read_text().splitlines() if json.loads(x)['id']=='uv-26')
        self.binding=json.loads((self.data/'evidence/scripts/uv-26.binding.json').read_text());old=SNAPSHOT.relative_to(scorecard.ROOT).as_posix();new=self.data.relative_to(scorecard.ROOT).as_posix()
        self.row['toolchain_evidence']['path']=self.row['toolchain_evidence']['path'].replace(old,new,1)
        for run in self.row['runs'].values():
            for field in ('stdout_path','stderr_path','effects_manifest_path'):
                if field in run:run[field]=run[field].replace(old,new,1)
        self.binding['artifacts']={k:v.replace(old,new,1) for k,v in self.binding['artifacts'].items()};self.entry={'files':{}}
        for loc in self.binding['artifacts'].values():self.rehash(scorecard.ROOT/loc)
    def file(self,role):return scorecard.ROOT/self.binding['artifacts'][role]
    def rehash(self,path):self.entry['files'][path.relative_to(scorecard.ROOT).as_posix()]=sha(path)
    def change(self,role,fn):
        p=self.file(role);d=json.loads(p.read_text());fn(d);dump(p,d);self.rehash(p)
    def strict(self):script_control.strict_case(scorecard,self.row,self.binding,self.entry)
    def strict_semantics(self):
        with patch.object(script_control,'env_seal'):self.strict()
    def trace_attack(self,mutator):
        p=self.file('O0_trace');trace=mutator(p.read_text());p.write_text(trace);self.rehash(p);self.change('O0_meta',lambda d:d.__setitem__('trace_sha256',sha(p)))
    def test_authentic_supplement_and_fresh_stock_controls(self):
        for tier in ('O0','O3'):
            m=json.loads(self.file(tier+'_meta').read_text());trace=self.file(tier+'_trace').read_text();smoke=json.loads(self.file(tier+'_mask_smoke').read_text());st=self.file(tier+'_mask_smoke_trace').read_text();b=json.loads(self.file(tier+'_build').read_text())['build'];policy=json.loads(self.file('policy').read_text())
            script_control.script_withdrawal(scorecard,'uv-26',m,trace,smoke,st,b,policy)
            for mode in ('direct','imported'):script_control.gate_control(scorecard,'uv-26',tier,mode,lambda role:self.file(role),b,script_control.BUILD_TUPLES['ruff-b'])
        self.strict()
    def test_rebound_source_preimage_strict_flag_and_tuple(self):
        source=self.file('source');source.write_bytes(source.read_bytes()+b'\n');self.rehash(source)
        with self.assertRaisesRegex(ValueError,'source or launcher'):self.strict()
        source.write_bytes(source.read_bytes()[:-1]);self.rehash(source);self.change('O0_meta',lambda d:d['preimages']['CHANGELOG.md'].__setitem__('sha256','0'*64))
        with self.assertRaisesRegex(ValueError,'sealed lane'):self.strict()
        with self.assertRaisesRegex(ValueError,'preimages'):self.strict_semantics()
        self.binding['strict_source_control']='direct-and-withdraw-v1'
        with self.assertRaisesRegex(ValueError,'strict source mode'):self.strict()
        self.binding['strict_source_control']='exact-bwrap-withdraw-v1';self.binding['build_tuple']='ruff_a';self.binding['compiler_sha256'],self.binding['runtime_sha256']=script_control.BUILD_TUPLES['ruff-a']
        for tier in ('O0','O3'):self.row['runs'][tier]['toolchain'].update(compiler_sha256=self.binding['compiler_sha256'],runtime_sha256=self.binding['runtime_sha256'])
        with self.assertRaisesRegex(ValueError,'fixed build tuple'):self.strict()
    def test_rebound_wrapper_open_exec_argv_and_env(self):
        original=self.file('O0_trace').read_text()
        attacks=[(lambda t:t.replace('--argv0=scripts/reverse-changelog.py','--argv0=other',1),'wrapper|supervisor'),
                 (lambda t:t.replace('"-u", "PWD"','"-u", "OTHER"',1),'wrapper|supervisor'),
                 (lambda t:self.insert_native(t,'openat(AT_FDCWD<{cwd}>, "/tmp/extensionless", O_RDONLY|O_CLOEXEC) = 40</tmp/extensionless>'),'unauthorized'),
                 (lambda t:self.insert_native(t,'execve("/usr/bin/python3", ["/usr/bin/python3"], 0x0 /* 0 vars */) = 0'),'unverified exec')]
        for attack,reason in attacks:
            self.trace_attack(attack)
            with self.subTest(reason=reason),self.assertRaisesRegex(ValueError,reason):self.strict_semantics()
            self.file('O0_trace').write_text(original);self.rehash(self.file('O0_trace'));self.change('O0_meta',lambda d:d.__setitem__('trace_sha256',sha(self.file('O0_trace'))))
        self.row['runs']['O0']['argv']=['other']
        with self.assertRaisesRegex(ValueError,'original arguments'):self.strict()
        self.row['runs']['O0']['argv']=['scripts/reverse-changelog.py','CHANGELOG.md'];self.row['runs']['O0']['environment']['HOME']='/tmp/other'
        with self.assertRaisesRegex(ValueError,'home role'):self.strict()
    def insert_native(self,trace,call):
        lines=trace.splitlines();target=raw_control.exec_sequence(lines,scorecard.require,4)[-1];lines.insert(target[0]+1,target[1]+' '+call.format(cwd=self.row['runs']['O0']['cwd']));return '\n'.join(lines)+'\n'
    def test_rebound_gate_child_and_generated_escape(self):
        for field,value in [('compiler_exit',0),('image_missing',False)]:
            self.change('direct_O0_json',lambda d:d.__setitem__(field,value))
            with self.subTest(field=field),self.assertRaisesRegex(ValueError,'gate child'):self.strict()
            self.change('direct_O0_json',lambda d:d.__setitem__(field,1 if field=='compiler_exit' else True))
        dep=self.file('cli_depfile');dep.write_text(dep.read_text().replace('out/imre-stdlib.zip','out/../unapproved.zip',1));self.rehash(dep);self.change('custody_setup',lambda d:d['product']['imre.d'].__setitem__('sha256',sha(dep)))
        with self.assertRaisesRegex(ValueError,'generated path custody'):self.strict()

    def test_compressed_gate_digest_cannot_be_rebound(self):
        p=self.file('direct_O0_trace');p.write_bytes(p.read_bytes()+b'changed');self.rehash(p)
        with self.assertRaisesRegex(ValueError,'compressed gate trace digest'):self.strict()

    def test_script_adapter_cannot_bypass_common_native_checks(self):
        self.row['runs']['O0']['build_argv']=['bad'];entry={'adapter':'script-control-v1'}
        with patch.object(scorecard,'activation_entry',return_value=entry),patch.object(script_control,'validate_record') as adapter:
            with self.assertRaisesRegex(ValueError,'exact native build invocation'):scorecard.validate_snapshot_record(self.row,'script',False,scorecard.require)
            adapter.assert_not_called()

    def test_authentic_eligible_rows_reach_adapter_only_after_common_checks(self):
        with patch.object(scorecard,'DATA_ROOT',SNAPSHOT),patch.object(scorecard,'activation_entry',return_value={'adapter':'script-control-v1'}),patch.object(script_control,'validate_record') as adapter:
            for id,shard in (('uv-10','uv-a'),('ty-05','ty')):
                row=next(json.loads(x) for x in (SNAPSHOT/'scripts'/(shard+'.jsonl')).read_text().splitlines() if json.loads(x)['id']==id)
                scorecard.validate_snapshot_record(row,'script',True,scorecard.require)
            self.assertEqual(adapter.call_count,2)

    def stock_semantics(self):
        file=raw_control.bound_artifacts(scorecard,self.entry,self.binding['artifacts'])
        script_control.fresh_stock_observation(scorecard,self.row,file)
    def stock_trace_attack(self,mutate):
        import gzip
        path=self.file('stock_trace');raw=mutate(gzip.decompress(path.read_bytes()).decode()).encode();path.write_bytes(gzip.compress(raw,mtime=0));self.rehash(path)
        self.change('stock_meta',lambda d:d.update(trace_sha256=hashlib.sha256(raw).hexdigest(),trace_bytes=len(raw)))
    def test_rebound_fresh_stock_before_and_source(self):
        path=self.file('stock_meta');original=path.read_bytes()
        for change,reason in ((lambda d:d['before']['CHANGELOG.md'].__setitem__('sha256','0'*64),'before snapshot'),
                              (lambda d:d['source'].__setitem__('path','scripts/other.py'),'source')):
            self.change('stock_meta',change)
            with self.subTest(reason=reason),self.assertRaisesRegex(ValueError,'sealed asset'):self.strict()
            with self.subTest(reason=reason),self.assertRaisesRegex(ValueError,reason):self.stock_semantics()
            path.write_bytes(original);self.rehash(path)
    def test_rebound_fresh_stock_raw_argv_and_environment(self):
        path=self.file('stock_trace');meta=self.file('stock_meta');trace_original=path.read_bytes();meta_original=meta.read_bytes()
        for change in (lambda t:t.replace('"CHANGELOG.md"], [','"OTHER.md"], [',1),lambda t:t.replace('TZ=UTC','TZ=OTHER',1)):
            self.stock_trace_attack(change)
            with self.assertRaisesRegex(ValueError,'sealed asset|raw trace'):self.strict()
            with self.assertRaisesRegex(ValueError,'raw argv, environment'):self.stock_semantics()
            path.write_bytes(trace_original);meta.write_bytes(meta_original);self.rehash(path);self.rehash(meta)
    def test_rebound_fresh_stock_must_open_original_source(self):
        source=self.row['runs']['stock']['cwd']+'/'+self.row['source']['path'];self.stock_trace_attack(lambda t:t.replace(source,'/tmp/unapproved.py'))
        with self.assertRaisesRegex(ValueError,'sealed asset|raw trace'):self.strict()
        with self.assertRaisesRegex(ValueError,'did not open the original source'):self.stock_semantics()
    def test_rebound_fresh_stock_case_and_effect_mapping(self):
        old=self.binding['artifacts']['stock_meta'];other=self.data/'evidence/scripts/stock-before/uv-29/result.json';self.binding['artifacts']['stock_meta']=other.relative_to(scorecard.ROOT).as_posix();self.rehash(other)
        with self.assertRaisesRegex(ValueError,'sealed asset or case'):self.strict()
        self.binding['artifacts']['stock_meta']=old;self.change('stock_meta',lambda d:d['after']['CHANGELOG.md'].__setitem__('sha256','0'*64))
        with self.assertRaisesRegex(ValueError,'sealed asset'):self.strict()
        with self.assertRaisesRegex(ValueError,'effect mapping'):self.stock_semantics()

if __name__=='__main__':unittest.main()
