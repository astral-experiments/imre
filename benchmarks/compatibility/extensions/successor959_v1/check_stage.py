"""Offline source gate: closed-environment audit and isolated descendant fixture."""
from __future__ import annotations
import copy
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile

HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[1]))
import scorecard,raw_control
from extensions.successor959_v1 import authority,generate


def run(*args,cwd=None):return subprocess.check_output(args,cwd=cwd,stderr=subprocess.PIPE)

def reject(call,label):
    try:call()
    except (ValueError,KeyError,subprocess.CalledProcessError):return
    raise AssertionError('accepted rebound: '+label)


def closed_environment():
    binding=json.loads((HERE/'bindings/json.input.json').read_bytes());allowed={k:set(v) for k,v in binding['closed_environment'].items()};fields={'environment','actual_compiler_child_environment','supervisor_environment','build_environment_override','environment_override','env'};sensitive=re.compile(rb'(?:gh[pousr]_[A-Za-z0-9]{20,}|sk-(?:proj-)?[A-Za-z0-9_-]{20,}|AKIA[A-Z0-9]{16}|-----BEGIN [A-Z ]*PRIVATE KEY-----|(?i:authorization\s*[:=]\s*bearer\s+\S+))');counts=[0,0,0]
    def fail(path,category):raise ValueError(f'closed-environment audit failed: {path} ({category})')
    def environment(env,path):
        counts[1]+=1
        if any(k not in allowed or not isinstance(v,str) or v not in allowed[k] for k,v in env.items()):fail(path,'environment outside declared fixture allowlist')
    def walk(obj,path):
        if isinstance(obj,dict):
            for k,v in obj.items():
                if k in fields and isinstance(v,dict):environment(v,path)
                walk(v,path)
        elif isinstance(obj,list):
            for v in obj:walk(v,path)
    def inspect(data,path):
        counts[0]+=1
        if sensitive.search(data):fail(path,'credential-shaped content')
        if path.endswith(('.json','.jsonl')):
            try:
                for item in (data.splitlines() if path.endswith('.jsonl') else [data]):walk(json.loads(item),path)
            except (ValueError,UnicodeError):fail(path,'metadata parse or environment audit')
        if path.endswith('.trace'):
            for line in data.decode().splitlines():
                if 'execve(' not in line:continue
                counts[2]+=1
                try:
                    if not raw_control.EXEC.fullmatch(line):fail(path,'uninspectable raw exec')
                    env=raw_control.exec_environment(line,lambda ok,msg: (_ for _ in ()).throw(ValueError()) if not ok else None)
                except ValueError:fail(path,'uninspectable raw exec environment')
                environment(env,path)
    archive=HERE/'evidence/sealed/json-decode-959-selected-v1.tar.gz'
    for f in HERE.rglob('*'):
        if f.is_file() and f!=archive:inspect(f.read_bytes(),f.relative_to(HERE).as_posix())
    scorecard.require(archive.stat().st_size==42662 and raw_control.sha(archive.read_bytes())=='fae5b50a20c65bb58f960fc230491469b8e7266e6ff53f25e39305b4b72fe91b','sealed JSON archive differs')
    with tarfile.open(archive) as t:
        members=t.getmembers();scorecard.require(len(members)==71 and all(x.isreg() for x in members) and len({x.name for x in members})==len(members),'sealed JSON member inventory differs');raw={x.name:t.extractfile(x).read() for x in members}
        for name,data in raw.items():inspect(data,'archive/'+name)
        for loc in binding['roles'].values():
            f=scorecard.ROOT/loc;name=f.relative_to(HERE/'evidence/json').as_posix();scorecard.require(name in raw and raw[name]==f.read_bytes(),'selected role differs from sealed archive')
    return counts


def fixture_checks():
    scorecard.select_snapshot(authority.REVISION);m,old,_=authority.authority(scorecard);act=authority.activated_entries(scorecard);frozen=scorecard.load_frozen_scripts();names=scorecard.families();stdlib=scorecard.load_records('stdlib',names,frozen);scripts=scorecard.load_records('script',names,frozen);j=[r for r in stdlib if r['family']=='json'];fn=[r for r in stdlib if r['family']=='fnmatch'];scorecard.require(len(j)==30 and all(not r['paired'] and all(r['runs'][t]['outcome']=='pass' for t in scorecard.TIERS) for r in j) and len(fn)==24 and all(r['paired'] for r in fn) and len(scripts)==4,'actual legacy or selected original outcomes differ')
    old_hashes={}
    for group in ('families','script_shards'):
        for entry in old[group].values():
            loc=entry['receipt'];public=raw_control.git(scorecard.ROOT,'show','--no-textconv',authority.ACTIVATION_COMMIT+':'+loc);scorecard.require(authority.safe_path(scorecard,loc).read_bytes()==public,'old receipt bytes differ');old_hashes[loc]=raw_control.sha(public)
    entry=act['families']['json'];binding=json.loads(authority.artifact(scorecard,entry,entry['binding']).read_bytes());cohort=next(iter(binding['cohorts'].values()))
    with tempfile.TemporaryDirectory(prefix='successor959-generator-') as tmp:
        dest=Path(tmp)/'json.jsonl';generate.finite(scorecard,binding,dest);scorecard.require(dest.read_bytes()==authority.artifact(scorecard,entry,entry['staged_receipt']).read_bytes(),'generator differs from immutable observed receipt')
    ntests=0
    def bad(call,label):
        nonlocal ntests
        reject(call,label);ntests+=1
    n=copy.deepcopy(m);n['predecessor']['sha256']='0'*64;bad(lambda:authority.verify_stage(scorecard,n),'predecessor')
    n=copy.deepcopy(m);n['inputs']['stdlib/extra']=next(iter(n['inputs'].values()));bad(lambda:authority.verify_stage(scorecard,n),'unbound input')
    n=copy.deepcopy(m);n['bindings']['stdlib']['json']['binding']=authority.MANIFEST;bad(lambda:authority.verify_stage(scorecard,n),'wrong binding ref')
    bad(lambda:authority.artifact(scorecard,entry,m['patches'][0]['path']),'unrelated shared-map path');bad(lambda:authority.donor(scorecard,entry,{'cli':'0'*64,'runtime':'0'*64,'clang':'0'*64}),'cross tuple')
    patch=authority.safe_path(scorecard,m['patches'][0]['path']);saved=patch.read_bytes()
    try:
        patch.write_bytes(saved+b'--- a/README.md\n+++ b/README.md\n@@ -0,0 +1 @@\n+unapproved\n');n=copy.deepcopy(m);digest=raw_control.sha(patch.read_bytes());n['files'][m['patches'][0]['path']]=n['patches'][0]['sha256']=digest;bad(lambda:authority.verify_stage(scorecard,n),'rehashed extra patch path')
    finally:patch.write_bytes(saved)
    prior=authority.safe_path(scorecard,old['families']['fnmatch']['receipt']);saved=prior.read_bytes()
    try:prior.write_bytes(saved+b'\n');bad(lambda:authority.verify_stage(scorecard,m),'old receipt')
    finally:prior.write_bytes(saved)
    for loc,label in ((authority.MANIFEST,'public manifest'),(m['patches'][0]['files'][0]['path'],'patched source')):
        p=authority.safe_path(scorecard,loc);saved=p.read_bytes()
        try:p.write_bytes(saved+b' ');authority._authority.cache_clear();bad(lambda:authority.authority(scorecard),label)
        finally:p.write_bytes(saved);authority._authority.cache_clear()
    receipt=authority.safe_path(scorecard,entry['receipt']);ap=authority.safe_path(scorecard,authority.BASE+'/activation.json');saved=receipt.read_bytes();saved_ap=ap.read_bytes()
    for label,change in (('rehashed source',lambda rows:rows[0]['source'].update(sha256='0'*64)),('duplicate occurrence',lambda rows:rows.append(copy.deepcopy(rows[0])))):
        try:
            rows=[json.loads(x) for x in saved.splitlines()];change(rows);receipt.write_text(''.join(json.dumps(x)+'\n' for x in rows));a=json.loads(saved_ap);a['receipt_hashes'][entry['receipt']]=raw_control.sha(receipt.read_bytes());ap.write_text(json.dumps(a));authority.activated_entries.cache_clear();bad(lambda:authority.activated_entries(scorecard),label)
        finally:receipt.write_bytes(saved);ap.write_bytes(saved_ap);authority.activated_entries.cache_clear()
    def adapt(overrides=None,control=None,swap=None):
        overrides=overrides or {};swap=swap or {};file=lambda r:authority.artifact(scorecard,entry,binding['roles'][swap.get(r,r)]);data=lambda r:overrides[r] if r in overrides else file(r).read_bytes();text=lambda r:data(r).decode();obj=lambda r:json.loads(data(r));return raw_control.adapt_selected_finite(scorecard,'json',binding,control or cohort,binding['source'],file,data,text,obj,entry)
    enc=lambda x:json.dumps(x,sort_keys=True).encode();read=lambda role:authority.artifact(scorecard,entry,binding['roles'][role]).read_bytes()
    for a,b in (('direct_launcher','imported_launcher'),('gate_O0_direct_trace','gate_O0_imported_trace'),('O0_build_json','O3_build_json')):bad(lambda a=a,b=b:adapt(swap={a:b,b:a}),'role swap '+a)
    for mode,attack in (('direct','environment'),('imported','poison-root')):
        prefix='gate_O0_'+mode;gm=json.loads(read(prefix+'_meta'));gt=read(prefix+'_trace').decode();lines=gt.splitlines(keepends=True);_,child=raw_control.execs(gt.splitlines(),scorecard.require)
        if attack=='environment':lines[child[0]]=lines[child[0]].replace('PYTHON_COLORS=0','PYTHON_COLORS=1');gm['actual_compiler_child_environment']['PYTHON_COLORS']='1';changed=''.join(lines)
        else:oldpath=gm['poison_path'];gm['poison_path']='/tmp/unrelated/'+Path(oldpath).name;changed=gt.replace(oldpath,gm['poison_path'])
        gm['trace_sha256']=raw_control.sha(changed.encode());bad(lambda:adapt({prefix+'_meta':enc(gm),prefix+'_trace':changed.encode()}),'rehashed gate '+attack)
    for mode in ('executable','environment','hash'):
        ctl=copy.deepcopy(cohort)
        if mode=='hash':assessment=json.loads(read('assessment'));assessment['stock_python_sha256']=ctl['stock_interpreter']['sha256']='0'*64;overrides={'assessment':enc(assessment)}
        else:r=json.loads(read('run'));meta=r['runs']['stock'];trace=read('stock_trace').decode();trace=trace.replace(meta['argv'][0],'/tmp/unapproved-python') if mode=='executable' else trace.replace('PYTHON_COLORS=0','PYTHON_COLORS=1',1);meta['trace_sha256']=raw_control.sha(trace.encode());overrides={'run':enc(r),'stock_meta':enc(meta),'stock_trace':trace.encode()}
        bad(lambda:adapt(overrides,ctl),'rehashed stock '+mode)
    bad(lambda:raw_control.finite_gate_delta(read('positive_launcher'),read('imported_launcher')+b'\n',binding['gate_operations']['imported'],scorecard.require),'launcher delta')
    gm=json.loads(read('gate_O0_direct_meta'));gm['image_absent_before']=False;bad(lambda:adapt({'gate_O0_direct_meta':enc(gm)}),'no-image before')
    row=copy.deepcopy(j[0]);row['paired']=True;bad(lambda:scorecard.validate_record(row,'stdlib',names,frozen),'identity-blocked pair')
    bp=authority.safe_path(scorecard,entry['binding']);saved=bp.read_bytes()
    for label,change in (('canonical denominator',lambda x:x.update(canonical_denominator=215)),('role collision',lambda x:x['roles'].update(direct_launcher=x['roles']['imported_launcher']))):
        try:x=json.loads(saved);change(x);bp.write_text(json.dumps(x));n=copy.deepcopy(m);n['files'][entry['binding']]=raw_control.sha(bp.read_bytes());bad(lambda:authority.verify_stage(scorecard,n),label)
        finally:bp.write_bytes(saved)
    r0=json.loads(read('run'));trace=read('O0_trace').decode();lines=trace.splitlines(keepends=True);parent,child=raw_control.execs(trace.splitlines(),scorecard.require)
    for label in ('delayed withdrawal','early restoration','unapproved exec'):
        changed=lines.copy()
        if label=='delayed withdrawal':i=next(i for i,x in enumerate(changed) if 'rename(' in x);line=changed.pop(i);changed.insert(child[0]+1,line)
        elif label=='early restoration':i=next(i for i,x in enumerate(changed[child[0]+1:],child[0]+1) if 'rename(' in x);line=changed.pop(i);changed.insert(child[0]+1,line)
        else:changed.insert(child[0]+1,f'{child[1]} execve("/bin/true", ["/bin/true"], []) = 0\n')
        text=''.join(changed);r=copy.deepcopy(r0);r['runs']['O0']['trace_sha256']=raw_control.sha(text.encode());bad(lambda:adapt({'run':enc(r),'O0_meta':enc(r['runs']['O0']),'O0_trace':text.encode()}),label)
    print(f'focus: {ntests} passed, 0 skipped; JSON 30 observed passes/0 strict, legacy fnmatch24 and scripts4 unchanged; old receipt hashes '+json.dumps(old_hashes,sort_keys=True))


def probe():
    counts=closed_environment();manifest=json.loads((HERE/'manifest.json').read_bytes());root=scorecard.ROOT;reject(lambda:authority.fixed_pin(scorecard),'source-stage self-authorization');base=run('git','merge-base','HEAD','main',cwd=root).decode().strip()
    with tempfile.TemporaryDirectory(prefix='successor959-source-',dir='/workspace/scratch') as temp:
        fixture=Path(temp)/'repo';g=lambda *a:run('git','-C',str(fixture),*a);run('git','clone','--quiet','--shared','--no-checkout',str(root),str(fixture));g('symbolic-ref','HEAD','refs/heads/fixture');g('update-ref','refs/heads/fixture',base);g('read-tree',base);g('remote','set-url','origin','https://github.com/astral-experiments/imre.git');g('config','user.name','Charlie Marsh');g('config','user.email','charlie.r.marsh@gmail.com');g('checkout',base,'--','benchmarks/compatibility','benchmarks/cpython_stdlib/full','README.md','docs/stdlib.md');shutil.copytree(HERE,fixture/authority.BASE);g('add','-f','--',authority.BASE);g('commit','--quiet','-m','temporary fixture source stage');stage=g('rev-parse','HEAD').decode().strip();raw=(fixture/authority.MANIFEST).read_bytes();pin={'commit':stage,'manifest':authority.MANIFEST,'git_blob':g('rev-parse',stage+':'+authority.MANIFEST).decode().strip(),'sha256':raw_control.sha(raw)}
        for patch in manifest['patches']:
            for item in patch['files']:scorecard.require(raw_control.sha((fixture/item['path']).read_bytes())==item['old_sha256'] and g('hash-object','--',item['path']).decode().strip()==item['old_git_blob'],'preimage differs')
            path=fixture/patch['path'];scorecard.require(raw_control.sha(path.read_bytes())==patch['sha256'],'patch differs');g('apply','--unidiff-zero','--check',str(path));g('apply','--unidiff-zero',str(path))
            for item in patch['files']:scorecard.require(raw_control.sha((fixture/item['path']).read_bytes())==item['post_sha256'],'postimage differs')
        pins=fixture/'benchmarks/compatibility/source_stage_pins.py';pins.write_text(pins.read_text()+'\nREVISION_959_SUCCESSOR = '+repr(pin)+'\n');activation={'schema':'imre.compatibility.successor-activation.v1','source_stage':pin,'families':manifest['bindings']['stdlib'],'receipt_hashes':{}}
        for entry in activation['families'].values():
            p=fixture/entry['receipt'];p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes((fixture/entry['staged_receipt']).read_bytes());activation['receipt_hashes'][entry['receipt']]=raw_control.sha(p.read_bytes())
        ap=fixture/authority.BASE/'activation.json';ap.write_text(json.dumps(activation,sort_keys=True)+'\n');files=[x['path'] for p in manifest['patches'] for x in p['files']]+[str(pins.relative_to(fixture)),str(ap.relative_to(fixture))]+list(activation['receipt_hashes']);g('add','-f','--',*files);g('commit','--quiet','-m','temporary fixture descendant');g('update-ref','refs/remotes/origin/main',g('rev-parse','HEAD').decode().strip());print(run(sys.executable,'-B',str(fixture/authority.BASE/'check_stage.py'),'--fixture',cwd=fixture).decode().strip())
    print(f'closed-environment audit: {counts[0]} files/member views, {counts[1]} environments, {counts[2]} raw execs; 0 violations')

if __name__=='__main__':fixture_checks() if '--fixture' in sys.argv else probe()
