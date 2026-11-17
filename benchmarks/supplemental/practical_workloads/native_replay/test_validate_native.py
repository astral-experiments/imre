"""Rebound mutations must never preserve the independently derived strict score."""
from __future__ import annotations
import contextlib, hashlib, io, json, os, re, shutil, subprocess, tempfile, unittest
from pathlib import Path
from unittest import mock
import validate_native as v

HERE=Path(__file__).resolve().parent; STOCK=HERE.parent/'stock_v1'; FIXTURE=Path(os.environ['NATIVE_REPLAY_EVIDENCE']).resolve() if os.environ.get('NATIVE_REPLAY_EVIDENCE') else None; REV=os.environ.get('NATIVE_REPLAY_REVISION'); ACT=os.environ.get('NATIVE_REPLAY_ACTIVATION_COMMIT'); ABLOB=os.environ.get('NATIVE_REPLAY_ACTIVATION_BLOB'); ASHA=os.environ.get('NATIVE_REPLAY_ACTIVATION_SHA256')
def put(p,x): p.write_text(json.dumps(x,indent=2,sort_keys=True)+'\n')
def edit(p,f): x=v.load(p); f(x); put(p,x)
def receipt(e,c='ruff-publisher',t='O0'): return e/c/t/'receipt.json'
def trace(e, change, c='ruff-publisher', t='O0', gate=False):
    o=e/c/t; p=o/('gate.trace' if gate else 'trace');p.write_text(change(p.read_text()));h=v.sha(p)
    if gate:
        edit(o/'gate.json',lambda g:g.update(trace_sha256=h));edit(receipt(e,c,t),lambda r:r.update(gate=v.load(o/'gate.json')))
    else:edit(receipt(e,c,t),lambda r:r['run'].update(trace_sha256=h))
def gate_file(e,name,data,key):
    o=e/'ruff-publisher/O0';p=o/name;p.write_bytes(data);edit(o/'gate.json',lambda g:g.update({key:v.sha(p)}));edit(receipt(e),lambda r:r.update(gate=v.load(o/'gate.json')))
def native_pid(text): return [m[1] for l in text.splitlines() if (m:=v.raw.EXEC.fullmatch(l))][1]
def inject(text,line,pid=None):
    rows=text.splitlines();pid=pid or native_pid(text);i=next(i for i,l in enumerate(rows) if l.startswith(pid+' ') and 'execve(' in l);rows.insert(i+1,line.format(pid=pid));return '\n'.join(rows)+'\n'
def change_missing(text): return re.sub(r'(newfstatat\(AT_FDCWD, "[^"]+publish-crates\.py", .*?)= -1 ENOENT \([^)]*\)',r'\1= 0',text,count=1)
def move_missing(text):
    rows=text.splitlines();i=next(i for i,l in enumerate(rows) if 'publish-crates.py"' in l and 'newfstatat(' in l and 'ENOENT' in l);line=rows.pop(i);j=next(j for j,l in enumerate(rows) if '.exe", [' in l and 'execve(' in l);rows.insert(j+1,line);return '\n'.join(rows)+'\n'
def gate_no_image(text):return re.sub(r'(newfstatat\(AT_FDCWD, "[^"]+(?:control\.exe|\.exe\.gate)", .*?)= -1 ENOENT \([^)]*\)',r'\1= 0',text,count=1)
def gate_order(text):
    rows=text.splitlines();i=next(i for i,l in enumerate(rows) if ('.exe.gate"' in l or 'control.exe"' in l) and 'newfstatat(' in l);line=rows.pop(i);j=next(j for j,l in enumerate(rows) if (m:=v.raw.EXEC.fullmatch(l)) and json.loads(m[2]).endswith('/release/imre'));rows.insert(j+1,line);return '\n'.join(rows)+'\n'
def fixture_hash(e):
    for p in (e/'ruff-publisher/O0').glob('fixture-*.sha256'):p.write_text(p.read_text().replace(v.load(e/'build.json')['stub_sha256'],'0'*64))
def fixture_path(e):
    o=e/'ruff-publisher/O0';r=v.load(receipt(e));old=r['run']['roles']['$CORPUS'];new=old+'-rebound'
    for name in ('receipt.json','fixture-before.sha256','fixture-after.sha256','trace'):
        p=o/name;p.write_text(p.read_text().replace(old,new))
    edit(receipt(e),lambda r:r['run'].update(trace_sha256=v.sha(o/'trace')))
def dotdot(text):
    calls=[m for l in text.splitlines() if (m:=v.raw.EXEC.fullmatch(l))];pid=calls[2][1];py=json.loads(calls[2][2]);p=str(Path(py).parent.parent/'lib/python3.14/../../../evil.py');return inject(text,f'{{pid}} openat(AT_FDCWD, "{p}", O_RDONLY) = -1 ENOENT (No such file or directory)',pid)
def omitted_archive(e):
    p=receipt(e,'uv-rich');r=v.load(p);a=r['build']['argv'];i=a.index('--extension-path');del a[i:i+2];put(p,r);trace(e,lambda s:'\n'.join(l for l in s.splitlines() if not ('rich-imports.zip"' in l and ('rename(' in l or 'newfstatat(' in l)))+'\n','uv-rich')
def duplicate_o0(e):shutil.rmtree(e/"ruff-publisher/O3");shutil.copytree(e/"ruff-publisher/O0",e/"ruff-publisher/O3")
def output(e):
    o=e/'ruff-publisher/O0';(o/'stdout').write_bytes(b'wrong\n');edit(receipt(e),lambda r:(r['run']['comparison_dimensions'].update(stdout=False),r['run'].update(stdout_sha256=v.sha(o/'stdout'))))
def effects(e):
    o=e/'ty-readme/O0';edit(o/'after.json',lambda x:x.append({'path':'wrong.txt','kind':'file','bytes':1,'mode':'0o600','sha256':'0'*64}));edit(o/'effects.normalized.json',lambda x:x['files']['created'].append({'path':'wrong.txt','kind':'file','bytes':1,'mode':'0o600','sha256':'0'*64}));edit(receipt(e,'ty-readme'),lambda r:r['run']['comparison_dimensions'].update(mutations=False))

@unittest.skipUnless(FIXTURE and REV and ACT and ABLOB and ASHA,"set future reviewed snapshot fixture, revision, and public activation commit")
class StrictReplay(unittest.TestCase):
    def check(self,e):
        with contextlib.redirect_stdout(io.StringIO()):return v.validate(e,STOCK,REV,ACT,ABLOB,ASHA)
    def test_baseline(self):self.assertEqual(self.check(FIXTURE)['total_cases'],len(v.load(STOCK/'sources.json')))
    def test_rebound_controls(self):
        def ident(k):return lambda e:edit(e/'build.json',lambda b:b.update({k:'0'*64}))
        def tr(f,**kw):return lambda e:trace(e,f,**kw)
        def fixture_exec(text):return text.replace('cargo_stub.py", "metadata"','other.py", "metadata"',1)
        controls={
          'CLI identity':ident('compiler_sha256'),'runtime identity':ident('runtime_sha256'),
          'image identity':lambda e:edit(receipt(e),lambda r:r['build'].update(image_sha256='0'*64)),
          'poison bytes':lambda e:gate_file(e,'gate.module.py.txt',b'status: int = 1\n','module_sha256'),
          'diagnostic':lambda e:gate_file(e,'gate.stderr',b'expected int, got str\n','stderr_sha256'),
          'gate no image':tr(gate_no_image,gate=True),'gate order':tr(gate_order,gate=True),
          'withdrawal ENOENT':tr(change_missing),'withdrawal order':tr(move_missing),
          'extra exec':tr(lambda s:inject(s,'{pid} execve("/bin/true", ["/bin/true"], 0x0 /* 0 vars */) = 0')),
          'extra child':tr(lambda s:inject(s,'{pid} clone(flags=SIGCHLD) = 999999')),
          'workload source':tr(lambda s:inject(s,'{pid} openat(AT_FDCWD, "/tmp/evil.py", O_RDONLY) = 3')),
          'relative dirfd':tr(lambda s:inject(s,'{pid} openat(999, "evil.py", O_RDONLY) = 3')),
          'dotdot fixture':tr(dotdot),'successful creat':tr(lambda s:inject(s,'{pid} creat("new.txt", 0600) = 3')),
          'io_uring':tr(lambda s:inject(s,'{pid} io_uring_setup(8, 0x0) = 3')),
          'fixture bytes':fixture_hash,'fixture rebound path':fixture_path,'fixture exec path':tr(fixture_exec),
          'altered output':output,'altered effects':effects,'omitted extension and ZIP':omitted_archive,'duplicate O0 as O3':duplicate_o0,
        }
        for name,mutate in controls.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temp:
                dest=Path(temp)/'snapshot';shutil.copytree(FIXTURE.parent,dest);e=dest/'evidence';mutate(e)
                try: result=self.check(e)
                except (ValueError,KeyError):continue
                self.assertNotEqual(result,self.check(FIXTURE),name)
    def test_case_independence(self):
        with tempfile.TemporaryDirectory() as temp:
            dest=Path(temp)/'snapshot';shutil.copytree(FIXTURE.parent,dest);fixture_hash(dest/'evidence');result=self.check(dest/'evidence');baseline=self.check(FIXTURE)
            self.assertFalse(result['cases']['ruff-publisher']['strict_pair']);self.assertEqual({k:x for k,x in result['cases'].items() if k!='ruff-publisher'},{k:x for k,x in baseline['cases'].items() if k!='ruff-publisher'})
    def test_archive_control_cannot_be_omitted(self):
        with tempfile.TemporaryDirectory() as temp:
            dest=Path(temp)/'snapshot';shutil.copytree(FIXTURE.parent,dest);omitted_archive(dest/'evidence');self.assertFalse(self.check(dest/'evidence')['cases']['uv-rich']['strict_pair'])

class SharedControls(unittest.TestCase):
    def test_shared_exec_default_is_two(self):
        lines=['1 execve("/probe", ["/probe"], 0x0) = 0','2 execve("/image", ["/image"], 0x0) = 0','3 execve("/other", ["/other"], 0x0) = 0']
        with self.assertRaises(ValueError):v.raw.execs(lines,v.need)
    def test_sensitive_open_controls(self):
        calls=[(0,'1','/probe',[]),(1,'2','/image',[]),(2,'3','/python/bin/python3.14',[])]
        for line in ['2 openat(999, "evil.py", O_RDONLY) = 3','3 openat(AT_FDCWD, "/python/lib/python3.14/../../../evil.py", O_RDONLY) = -1 ENOENT (No such file or directory)','2 creat("new.txt", 0600) = 3']:
            with self.subTest(line=line),self.assertRaises(ValueError):v.open_policy([line],'/work',calls,'/python/bin/python3.14','/fixture/cargo_stub.py',{'stdlib':{}})

POLICY_COMMIT='eacb0ab16a828c35dd3c86ed627d188fcedad8c7'
POLICY_PATH='benchmarks/supplemental/practical_workloads/fixture_policy_v1/policy.json'
POLICY_BLOB='a2aac2f74a0dce1b3f8ef0395d7e1ccb6080fc47'
POLICY_SHA='1e6049866e848aa52e04d886ef833604d6d4a785cc2286700d047385aa7df8ce'
POLICY_BINDING={'commit':POLICY_COMMIT,'path':POLICY_PATH,'blob':POLICY_BLOB,'sha256':POLICY_SHA}
REPO=STOCK.parents[3]

class SafetyRevision(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.raw=subprocess.check_output(['git','-C',str(REPO),'show',POLICY_COMMIT+':'+POLICY_PATH])
    def test_reviewed_public_policy_decodes(self):
        blob=subprocess.check_output(['git','-C',str(REPO),'rev-parse',POLICY_COMMIT+':'+POLICY_PATH],text=True).strip()
        self.assertEqual(blob,POLICY_BLOB);self.assertEqual(hashlib.sha256(self.raw).hexdigest(),POLICY_SHA)
        decoded=v.decode_policy(self.raw);self.assertEqual(len(decoded['stdlib']),25);self.assertIn('collections/__init__.py',decoded['stdlib'])
        self.assertEqual(v.policy_blob(REPO,POLICY_BINDING),decoded)
    def test_real_policy_open_boundary(self):
        policy=v.decode_policy(self.raw);calls=[(0,'1','/probe',[]),(1,'2','/image',[]),(2,'3','/private/bin/python3.14',[])];stub='/fixture/cargo_stub.py';base=[f'3 openat(AT_FDCWD, "{stub}", O_RDONLY) = 3']*2;root='/private/lib/python3.14/'
        def check(path):v.open_policy(base+[f'3 openat(AT_FDCWD, "{root+path}", O_RDONLY) = 3'],'/work',calls,'/private/bin/python3.14',stub,policy)
        check('collections/__init__.py')
        for path in ('attacker_payload.py','collections/__pycache__/__init__.cpython-314.pyc','../collections/__init__.py'):
            with self.subTest(path=path),self.assertRaises(ValueError):check(path)
        self.assertEqual(v.fixture_root('/private/bin/python3.14','/private','../lib/python3.14'),Path('/private/lib/python3.14'))
        with self.assertRaises(ValueError):v.fixture_root('/private/bin/python3.14','/other','../lib/python3.14')
    def test_fixture_directory_visibility_does_not_authorize_payload(self):
        policy=v.decode_policy(self.raw);calls=[(0,'1','/probe',[]),(1,'2','/image',[]),(2,'3','/private/bin/python3.14',[])];stub='/fixture/cargo_stub.py';root='/private/lib/python3.14';flags='O_RDONLY|O_NONBLOCK|O_CLOEXEC|O_DIRECTORY';base=[f'3 openat(AT_FDCWD, "{stub}", O_RDONLY) = 3']*2
        def directory(path,mode=flags,pid='3'):return f'{pid} openat(AT_FDCWD, "{path}", {mode}) = 7'
        def check(*rows):
            text='\n'.join(base+list(rows));rebound={'trace_sha256':hashlib.sha256(text.encode()).hexdigest()};self.assertEqual(v.raw.sha(text.encode()),rebound['trace_sha256']);v.open_policy(text.splitlines(),'/work',calls,'/private/bin/python3.14',stub,policy)
        for path in (root,root+'/json',root+'/encodings',root+'/re',root+'/pathlib',root+'/site-packages',root+'/lib-dynload'):check(directory(path))
        for row in (directory(root+'/unlisted'),directory(root+'/json','O_WRONLY|O_DIRECTORY'),directory(root+'/json',flags+'|O_UNKNOWN'),directory(root+'/json',pid='2'),directory(root+'/json/../json')):
            with self.subTest(row=row),self.assertRaises(ValueError):check(row)
        with self.assertRaises(ValueError):check(f'3 openat(AT_FDCWD, "{root}/lib-dynload/attacker.so", O_RDONLY) = -1 ENOENT (No such file or directory)')
        check(directory(root+'/json'),directory(root+'/json'))
        with self.assertRaises(ValueError):check(directory(root+'/json'),f'3 openat(7, ".", O_RDONLY) = 8')
        with self.assertRaises(ValueError):check(directory(root+'/json'),f'3 openat(7, ".", O_WRONLY|O_DIRECTORY) = 8')
        for payload in ('unapproved.py','unapproved.so','unapproved.whl'):
            with self.subTest(payload=payload),self.assertRaises(ValueError):check(directory(root+'/json'),f'3 openat(7, "{payload}", O_RDONLY) = 8')
    def test_approved_fixture_paths_are_read_only(self):
        policy=v.decode_policy(self.raw);calls=[(0,'1','/probe',[]),(1,'2','/image',[]),(2,'3','/private/bin/python3.14',[])];stub='/fixture/cargo_stub.py';source='/private/lib/python3.14/collections/__init__.py'
        def check(path,flags):
            rows=[f'3 openat(AT_FDCWD, "{stub}", {f}) = 3' for f in ('O_RDONLY','O_RDONLY|O_CLOEXEC')]+[f'3 openat(AT_FDCWD, "{path}", {flags}) = 4'];v.open_policy(rows,'/work',calls,'/private/bin/python3.14',stub,policy)
        for flags in ('O_RDONLY','O_RDONLY|O_CLOEXEC'):check(source,flags)
        for path,flags in [(source,'O_WRONLY|O_TRUNC'),(stub,'O_WRONLY'),(source,'O_RDWR'),(source,'O_RDONLY|O_APPEND'),(source,'O_RDONLY|O_CREAT'),(source,'O_RDONLY|O_TRUNC'),(stub,'O_RDONLY|O_CLOEXEC|O_UNKNOWN')]:
            with self.subTest(path=path,flags=flags),self.assertRaises(ValueError):check(path,flags)
    def test_only_fixture_derived_cache_enoent_is_permitted(self):
        policy=v.decode_policy(self.raw);calls=[(0,'1','/probe',[]),(1,'2','/image',[]),(2,'3','/private/bin/python3.14',[])];stub='/fixture/cargo_stub.py';base=[f'3 openat(AT_FDCWD, "{stub}", O_RDONLY) = 3']*2;cache='/private/lib/python3.14/collections/__pycache__/__init__.cpython-314.pyc'
        def check(pid,path,result,dirfd='AT_FDCWD',flags='O_RDONLY|O_CLOEXEC'):v.open_policy(base+[f'{pid} openat({dirfd}, "{path}", {flags}) = {result}'],'/work',calls,'/private/bin/python3.14',stub,policy)
        check('3',cache,'-1 ENOENT (No such file or directory)')
        with self.assertRaises(ValueError):check('3',cache,'-1 ENOENT (No such file or directory)',flags='O_WRONLY|O_CREAT')
        with self.assertRaises(ValueError):check('3','/private/lib/libpython3.14.so','-1 ENOENT (No such file or directory)')
        for pid,path,result,fd in [('3',cache,'3','AT_FDCWD'),('3',cache,'-1 EACCES (Permission denied)','AT_FDCWD'),('2',cache,'-1 ENOENT (No such file or directory)','AT_FDCWD'),('3',cache.replace('__init__','attacker_payload'),'-1 ENOENT (No such file or directory)','AT_FDCWD'),('3',cache.replace('cpython-314','cpython-313'),'-1 ENOENT (No such file or directory)','AT_FDCWD'),('3','collections/__pycache__/__init__.cpython-314.pyc','-1 ENOENT (No such file or directory)','999'),('3',cache,'-1 ENOENT (No such file or directory)','999')]:
            with self.subTest(pid=pid,path=path,result=result,fd=fd),self.assertRaises(ValueError):check(pid,path,result,fd)
    def test_capture_helpers_are_bound_to_public_squash(self):
        commit='a'*40;producer=b'as-used producer';helper=b'as-used helper';build={'producer_sha256':hashlib.sha256(producer).hexdigest(),'validator_sha256':hashlib.sha256(helper).hexdigest()};base='benchmarks/supplemental/practical_workloads/native_replay/';files={commit+':'+base+'produce.py':producer,commit+':'+base+'validate_native.py':helper}
        with mock.patch.object(v,'public_commit') as public,mock.patch.object(v,'git',side_effect=lambda repo,op,spec:files[spec]):
            v.public_capture_sources(REPO,commit,build);public.assert_called_with(REPO,commit)
            for key in ('producer_sha256','validator_sha256'):
                with self.subTest(key=key),self.assertRaises(ValueError):v.public_capture_sources(REPO,commit,{**build,key:'0'*64})
        with mock.patch.object(v,'public_commit',side_effect=ValueError('not public')):
            with self.assertRaises(ValueError):v.public_capture_sources(REPO,commit,build)
    def test_policy_rebound_cannot_replace_reviewed_blob(self):
        for key,value in [('path',POLICY_PATH.replace('policy.json','README.md')),('blob','0'*40),('sha256','0'*64)]:
            with self.subTest(key=key),self.assertRaises(ValueError):v.policy_blob(REPO,{**POLICY_BINDING,key:value})
        changed=json.loads(self.raw);changed['stdlib']['files_sha256']['attacker_payload.py']='0'*64
        with self.assertRaises(ValueError):v.decode_policy(json.dumps(changed).encode())
    def test_public_activation_and_receipt_identity_are_fixed(self):
        revision="d26f19448c70c00039c305f105a85c215f820577";record,policy=v.activation(REPO,revision,v.ACTIVATION_COMMIT,v.ACTIVATION_BLOB,v.ACTIVATION_SHA)
        self.assertEqual(record["strict_credit"],0);self.assertEqual(len(policy["stdlib"]),25)
        for commit,blob,digest in [("0"*40,v.ACTIVATION_BLOB,v.ACTIVATION_SHA),(v.ACTIVATION_COMMIT,"0"*40,v.ACTIVATION_SHA),(v.ACTIVATION_COMMIT,v.ACTIVATION_BLOB,"0"*64)]:
            with self.assertRaises(ValueError):v.activation(REPO,revision,commit,blob,digest)
        with tempfile.TemporaryDirectory() as temp:
            e=Path(temp)/"evidence";(e/"identity").mkdir(parents=True);put(e/"build.json",{});put(e/"identity/commands.json",{});p=e/"identity/artifacts.sha256";p.write_text("0"*64+"  /old\n");activated={"artifact_identity_sha256":v.sha(p)};p.write_text("1"*64+"  /attacker-rehashed\n")
            with self.assertRaisesRegex(ValueError,"reviewed native revision/artifact identity"):v.artifact_ids(e,STOCK,revision,activated,v.ACTIVATION_COMMIT)
    def test_fresh_clone_local_attributes_override_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            repo=Path(temp)/"repo";subprocess.run(["git","init","-q",str(repo)],check=True);(repo/".git/info/attributes").write_text("* filter=attacker\n")
            with self.assertRaisesRegex(ValueError,"local info/attributes"):v.tracked_attributes(repo,"a"*40,v.CRLF_PATHS)
    def test_frozen_gate_injections(self):
        sources=v.load(STOCK/'sources.json');marker=b'from __future__ import annotations\n';import_line=b'import architecture_control\n'
        for case in ('ruff-publisher','uv-publisher','uv-rich'):
            row=sources[case];original=(STOCK/row['vendored']).read_bytes();expected=import_line+original if case=='uv-rich' else original.replace(marker,marker+import_line,1)
            with self.subTest(case=case):
                self.assertEqual(v.gate_launcher(original,row['sha256']),expected)
                with self.assertRaises(ValueError):v.gate_launcher(original+b'# tampered\n',row['sha256'])
        publisher=(STOCK/sources['ruff-publisher']['vendored']).read_bytes();invalid=[publisher.replace(marker,marker+marker),b'x=1\n'+marker,b'from __future__ import division\n',b'"""docstring"""\nx=1\n']
        for source in invalid:
            with self.subTest(source=source[:30]),self.assertRaises(ValueError):v.gate_launcher(source,hashlib.sha256(source).hexdigest())
    def test_build_provenance_requires_recompiled_exact_crates(self):
        b={'cargo_build':{'cwd':'/source'},'compiler':'/target/release/imre','runtime':'/target/release/libimre_runtime.a'};log='Compiling imre-compiler v1 (/source/crates/imre-compiler)\nCompiling imre-runtime v1 (/source/crates/imre-runtime)';cd=b'/target/release/imre: /source/crates/imre-compiler/src/main.rs\n';rd=b'/target/release/libimre_runtime.a: /source/crates/imre-runtime/src/lib.rs\n';v.prebuild_provenance(b,log,cd,rd)
        for badlog,badc,badr in [(log.replace('Compiling imre-compiler','Fresh imre-compiler'),cd,rd),(log,cd.replace(b'/source/',b'/other/'),rd),(log,cd,rd.replace(b'/source/',b'/other/'))]:
            with self.assertRaises(ValueError):v.prebuild_provenance(b,badlog,badc,badr)


NATIVE_REV="d26f19448c70c00039c305f105a85c215f820577"
class StageCustody(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.commit=v.ACTIVATION_COMMIT;cls.evidence=HERE.parent/"native_d26_v3/evidence"
        cls.record=json.loads(v.git(REPO,"show",cls.commit+":benchmarks/supplemental/practical_workloads/activations/"+NATIVE_REV+".json"))
    def check(self,e,record):return v.artifact_ids(e,STOCK,NATIVE_REV,record,self.commit)
    def test_full_artifact_ids_public_stage(self):
        self.assertEqual(self.record["strict_credit"],0);self.assertEqual(self.check(self.evidence,self.record)[0]["pin"],NATIVE_REV)
    def test_rehashed_source_controls(self):
        def dep(e,b,a,p,key,old,new):
            path=e/"identity"/(key+".d");data=path.read_bytes().replace(old.encode(),new.encode(),1);path.write_bytes(data);digest=v.raw.sha(data)
            b["depfiles"][key]["sha256"]=digest;a["depfiles"][key]["raw_sha256"]=digest;p["depfiles"][key]["raw_depfile_sha256"]=digest
        def escape(e,b,a,p,ids):dep(e,b,a,p,"compiler","../../benchmarks/","../../../../escape/")
        def source(e,b,a,p,ids):dep(e,b,a,p,"runtime","/src/lib.rs","/src/injected.rs")
        def unknown(e,b,a,p,ids):
            row=a["generated"][0];old=row["path"];row["path"]=old.replace("imre-stdlib-sources.rs","unreviewed.rs");ids[row["path"]]=ids.pop(old);dep(e,b,a,p,"compiler",old,row["path"])
        def proof_field(field,value):return lambda e,b,a,p,ids:p["transforms"][0].update({field:value})
        def attributes(e,b,a,p,ids):p["transforms"][0]["attributes"]["filter"]="attacker"
        def attr_blob(e,b,a,p,ids):p["tracked_gitattributes"]["content_sha256"]="0"*64
        controls={"real escape":escape,"untracked source":source,"unknown generated":unknown,"raw transform bytes":proof_field("raw_sha256","0"*64),"canonical OID":proof_field("canonical_blob_oid","0"*40),"canonical content":proof_field("canonical_content_sha256","0"*64),"attributes":attributes,"tracked attributes blob":attr_blob}
        for label,mutate in controls.items():
            with self.subTest(control=label),tempfile.TemporaryDirectory(prefix=".score-rebound-",dir=REPO) as temp:
                snapshot=Path(temp)/"snapshot";shutil.copytree(self.evidence.parent,snapshot);e=snapshot/"evidence";b=v.load(e/"build.json");a=v.load(e/"depfile_audit.json");p=v.load(e/"depfile_crlf_proof.json");identity=e/"identity/artifacts.sha256";ids=v.raw.sha_lines(identity.read_text())
                mutate(e,b,a,p,ids);put(e/"depfile_crlf_proof.json",p);b["depfile_correction_proof_sha256"]=v.sha(e/"depfile_crlf_proof.json");a["audit_history"]["correction_proof_sha256"]=b["depfile_correction_proof_sha256"];put(e/"depfile_audit.json",a);b["depfile_audit_sha256"]=v.sha(e/"depfile_audit.json");put(e/"build.json",b)
                for key in ("depfile_audit","depfile_correction_proof"):ids[b[key]]=b[key+"_sha256"]
                ids[b["manifest_path"]]=v.sha(e/"build.json");identity.write_text("".join(digest+"  "+path+"\n" for path,digest in ids.items()));record={**self.record,"artifact_identity_sha256":v.sha(identity)}
                with self.assertRaises(ValueError):self.check(e,record)
    def test_rehashed_checkpoint_cannot_change_original_build(self):
        with tempfile.TemporaryDirectory(prefix=".score-rebound-",dir=REPO) as temp:
            snapshot=Path(temp)/"snapshot";shutil.copytree(self.evidence.parent,snapshot);e=snapshot/"evidence";b=v.load(e/"build.json");original=e/"capture_checkpoint_7b18276/build.json";edit(original,lambda x:x.update(compiler_sha256="0"*64));custody=e/"capture_checkpoint_custody.json";obj=v.load(custody);row=next(x for x in obj["original_files"] if x["archived_path"].endswith("/build.json"));row["sha256"]=v.sha(original);row["original_blob_oid"]=subprocess.check_output(["git","hash-object","--stdin"],input=original.read_bytes()).decode().strip();put(custody,obj)
            identity=e/"identity/artifacts.sha256";ids=v.raw.sha_lines(identity.read_text());base=Path(b["manifest_path"]).parent;ids[str(base/"capture_checkpoint_7b18276/build.json")]=v.sha(original);ids[str(base/custody.name)]=v.sha(custody);identity.write_text("".join(digest+"  "+path+"\n" for path,digest in ids.items()));record={**self.record,"artifact_identity_sha256":v.sha(identity),"retrospective_checkpoint_custody_sha256":v.sha(custody)}
            with self.assertRaisesRegex(ValueError,"original checkpoint identity"):self.check(e,record)
    def test_rehashed_identity_cannot_replace_external_activation(self):
        with tempfile.TemporaryDirectory(prefix=".score-rebound-",dir=REPO) as temp:
            snapshot=Path(temp)/"snapshot";shutil.copytree(self.evidence.parent,snapshot);identity=snapshot/"evidence/identity/artifacts.sha256";identity.write_text(identity.read_text().replace(v.load(snapshot/"evidence/build.json")["compiler_sha256"],"0"*64,1))
            with self.assertRaisesRegex(ValueError,"reviewed native revision/artifact identity"):self.check(snapshot/"evidence",self.record)

if __name__=='__main__':unittest.main()
