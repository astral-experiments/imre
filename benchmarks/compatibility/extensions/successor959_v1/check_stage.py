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
    binding=json.loads((HERE/'bindings/json.input.json').read_bytes());allowed={k:set(v) for k,v in binding['closed_environment'].items()};fields={'environment','actual_compiler_child_environment','actual_compiler_environment','actual_child_environment','supervisor_environment','build_environment_override','environment_override','env'};sensitive=re.compile(rb'(?:gh[pousr]_[A-Za-z0-9]{20,}|sk-(?:proj-)?[A-Za-z0-9_-]{20,}|AKIA[A-Z0-9]{16}|-----BEGIN [A-Z ]*PRIVATE KEY-----|(?i:authorization\s*[:=]\s*bearer\s+\S+))');counts=[0,0,0]
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
                    if line.endswith('] <unfinished ...>'):line=line.removesuffix(' <unfinished ...>')+') = 0'
                    if not raw_control.EXEC.fullmatch(line):fail(path,'uninspectable raw exec')
                    env=raw_control.exec_environment(line,lambda ok,msg: (_ for _ in ()).throw(ValueError()) if not ok else None)
                except ValueError:fail(path,'uninspectable raw exec environment')
                environment(env,path)
    archive=HERE/'evidence/sealed/json-decode-959-selected-v1.tar.gz'
    delta=json.loads((HERE/'bindings/json.identity.json').read_bytes());sealed={scorecard.ROOT/x for x in delta['archives']}
    for f in HERE.rglob('*'):
        if f.is_file() and f!=archive and f not in sealed:inspect(f.read_bytes(),f.relative_to(HERE).as_posix())
    scorecard.require(archive.stat().st_size==42662 and raw_control.sha(archive.read_bytes())=='fae5b50a20c65bb58f960fc230491469b8e7266e6ff53f25e39305b4b72fe91b','sealed JSON archive differs')
    with tarfile.open(archive) as t:
        members=t.getmembers();scorecard.require(len(members)==71 and all(x.isreg() for x in members) and len({x.name for x in members})==len(members),'sealed JSON member inventory differs');raw={x.name:t.extractfile(x).read() for x in members}
        for name,data in raw.items():inspect(data,'archive/'+name)
        for loc in binding['roles'].values():
            f=scorecard.ROOT/loc;name=f.relative_to(HERE/'evidence/json').as_posix();scorecard.require(name in raw and raw[name]==f.read_bytes(),'selected role differs from sealed archive')
    for arc,cfg in delta['archives'].items():
        members=authority.archive_members(scorecard,arc,json.dumps(cfg,sort_keys=True))
        for name,data in members.items():
            if name.endswith('.tar.gz'):
                scorecard.require(raw_control.sha(data)==raw_control.sha(archive.read_bytes()),'archived predecessor selection differs');continue
            inspect(data,Path(arc).name+'!'+name)
    return counts


def rebound_checks(m,old,entry,binding,rows,names,frozen):
    """Rebound authenticated observations after updating their immediate metadata hashes."""
    count=0
    def bad(call,label):
        nonlocal count
        reject(call,label);count+=1
    def read(role):
        loc=binding['base_roles'][role[6:]] if role.startswith('prior_') else binding['roles'][role]
        return authority.read(scorecard,entry,loc)
    enc=lambda x:json.dumps(x,sort_keys=True,separators=(',',':')).encode()
    cohort=next(iter(binding['cohorts'].values()))
    def adapt(over=None,control=None,swap=None):
        over=over or {};swap=swap or {};data=lambda role:over[role] if role in over else read(swap.get(role,role));text=lambda role:data(role).decode();obj=lambda role:json.loads(data(role));file=lambda role:authority.artifact(scorecard,entry,binding['roles'][role]);return raw_control.adapt_selected_finite(scorecard,binding['family'],binding,control or cohort,binding['source'],file,data,text,obj,entry)
    for field in ('predecessor','predecessor_stage'):
        n=copy.deepcopy(m);n[field]['sha256']='0'*64;bad(lambda:authority.verify_stage(scorecard,n),field)
    bad(lambda:authority.donor(scorecard,entry,dict(cli='0'*64,runtime='0'*64,clang='0'*64)),'substituted donor')
    bad(lambda:authority.digest(scorecard,entry,binding['roles']['stock_meta'].split('!',1)[0]+'!unapproved.py'),'unbound archive member')
    patch=authority.safe_path(scorecard,m['patches'][0]['path']);saved=patch.read_bytes()
    try:
        patch.write_bytes(saved+b'--- a/README.md\n+++ b/README.md\n@@ -0,0 +1 @@\n+unapproved\n');n=copy.deepcopy(m);n['files'][m['patches'][0]['path']]=n['patches'][0]['sha256']=raw_control.sha(patch.read_bytes());bad(lambda:authority.verify_stage(scorecard,n),'rehashed extra patch path')
    finally:patch.write_bytes(saved)
    prior=authority.safe_path(scorecard,old['families']['fnmatch']['receipt']);saved=prior.read_bytes()
    try:prior.write_bytes(saved+b'\n');bad(lambda:authority.verify_stage(scorecard,m),'old receipt bytes')
    finally:prior.write_bytes(saved)
    patched=authority.safe_path(scorecard,m['patches'][0]['files'][0]['path']);saved=patched.read_bytes()
    try:patched.write_bytes(saved+b' ');bad(lambda:authority.verify_stage(scorecard,m),'public successor source')
    finally:patched.write_bytes(saved)
    for tier in ('stock','O0'):
        row=copy.deepcopy(rows[0]);base=[json.loads(x) for x in authority.safe_path(scorecard,'benchmarks/compatibility/extensions/successor959_v1/templates/stdlib/json.jsonl').read_bytes().splitlines()][0];row['runs'][tier]=base['runs'][tier];bad(lambda:scorecard.validate_record(row,'stdlib',names,frozen),'baseline '+tier+' argv/raw credited against fresh identity')
    bp=authority.safe_path(scorecard,entry['binding']);saved=bp.read_bytes()
    try:
        delta=json.loads(saved);delta['replace_roles']['stock_meta']=delta['replace_roles']['O0_meta'];bp.write_bytes(enc(delta));n=copy.deepcopy(m);n['files'][entry['binding']]=raw_control.sha(bp.read_bytes());bad(lambda:authority.verify_stage(scorecard,n),'rehashed archive-member rebinding')
    finally:bp.write_bytes(saved)
    receipt=authority.safe_path(scorecard,entry['receipt']);ap=authority.safe_path(scorecard,authority.BASE+'/activation.json');saved=receipt.read_bytes();saved_ap=ap.read_bytes()
    try:
        changed=[json.loads(x) for x in saved.splitlines()];base=json.loads(authority.safe_path(scorecard,'benchmarks/compatibility/extensions/successor959_v1/templates/stdlib/json.jsonl').read_bytes().splitlines()[0]);changed[0]['runs']['O0']=base['runs']['O0'];receipt.write_bytes(b''.join(enc(x)+b'\n' for x in changed));a=json.loads(saved_ap);a['receipt_hashes'][entry['receipt']]=raw_control.sha(receipt.read_bytes());ap.write_bytes(enc(a));authority.activated_entries.cache_clear();bad(lambda:authority.activated_entries(scorecard),'rehashed baseline O0 receipt')
    finally:receipt.write_bytes(saved);ap.write_bytes(saved_ap);authority.activated_entries.cache_clear()
    for a,b in (('direct_launcher','imported_launcher'),('O0_build_json','O3_build_json'),('gate_O0_direct_trace','gate_O0_imported_trace')):
        bad(lambda a=a,b=b:adapt(swap={a:b,b:a}),'cross-role '+a)
    stock=json.loads(read('stock_meta'))
    for label,change in (('stock substituted hash',lambda x:[x[k].__setitem__('sha256','0'*64) for k in ('held_fd_pre','held_fd_post')]),('stock proc inode',lambda x:x['proc_exe_sample']['fstat'].__setitem__('ino',x['proc_exe_sample']['fstat']['ino']+1)),('stock post inode',lambda x:x['held_fd_post']['fstat'].__setitem__('ino',x['held_fd_post']['fstat']['ino']+1))):
        x=copy.deepcopy(stock);change(x);bad(lambda:adapt({'stock_meta':enc(x)}),label)
    for label in ('stock executable','stock environment'):
        x=copy.deepcopy(stock);trace=read('stock_trace').decode()
        if label.endswith('executable'):trace=trace.replace(x['argv'][0],'/tmp/substituted-python');x['argv'][0]='/tmp/substituted-python'
        else:trace=trace.replace('PYTHON_COLORS=0','PYTHON_COLORS=1',1);x['environment']['PYTHON_COLORS']='1'
        x['trace_sha256']=raw_control.sha(trace.encode());bad(lambda:adapt({'stock_meta':enc(x),'stock_trace':trace.encode()}),label)
    pos=json.loads(read('O0_build_json'))
    for label,change in (('compiler hash',lambda x:[x[p]['cli'].__setitem__('sha256','0'*64) for p in ('fd_before','fd_after')]),('runtime path',lambda x:[x[p]['runtime'].__setitem__('path','/tmp/runtime.a') for p in ('fd_before','fd_after')]),('post inode',lambda x:x['fd_after']['cli']['fstat'].__setitem__('ino',x['fd_after']['cli']['fstat']['ino']+1))):
        x=copy.deepcopy(pos);change(x);bad(lambda:adapt({'O0_build_json':enc(x)}),label)
    for label in ('compiler exec','compiler environment'):
        x=copy.deepcopy(pos);trace=read('O0_positive_trace').decode()
        if label.endswith('exec'):trace=trace.replace(x['actual_compiler_path'],'/tmp/substituted-imre',1)
        else:trace=trace.replace('PYTHON_COLORS=0','PYTHON_COLORS=1',1);x['actual_compiler_environment']['PYTHON_COLORS']='1'
        x['trace_sha256']=raw_control.sha(trace.encode());bad(lambda:adapt({'O0_build_json':enc(x),'O0_positive_trace':trace.encode()}),label)
    x=json.loads(read('O0_meta'));x['image_sha256']=pos['old_image_sha256'];bad(lambda:adapt({'O0_meta':enc(x)}),'stale O0 ELF')
    x=json.loads(read('O3_build_json'));x['old_image_sha256']='0'*64;bad(lambda:adapt({'O3_build_json':enc(x)}),'O3 content join')
    gm=json.loads(read('gate_O0_direct_meta'));br=json.loads(read('gate_bracket'))
    for label,change in (('gate compiler',lambda x:x.__setitem__('actual_compiler_path','/tmp/substituted-imre')),('gate environment',lambda x:x['actual_compiler_environment'].__setitem__('PYTHON_COLORS','1')),('gate no image',lambda x:x.__setitem__('image_absent_before',False))):
        x=copy.deepcopy(gm);change(x);b=copy.deepcopy(br);next(z for z in b['controls'] if z['tier']=='O0' and z['kind']=='direct')['record_sha256']=raw_control.sha(enc(x));bad(lambda:adapt({'gate_O0_direct_meta':enc(x),'gate_bracket':enc(b)}),label)
    raw=json.loads(read('O0_meta'));trace=read('O0_trace').decode();lines=trace.splitlines(keepends=True);_,child=raw_control.execs(trace.splitlines(),scorecard.require)
    for label in ('delayed withdrawal','early restoration','unapproved exec'):
        changed=lines.copy()
        if label=='delayed withdrawal':i=next(i for i,x in enumerate(changed) if 'rename(' in x);line=changed.pop(i);changed.insert(child[0]+1,line)
        elif label=='early restoration':i=next(i for i,x in enumerate(changed[child[0]+1:],child[0]+1) if 'rename(' in x);line=changed.pop(i);changed.insert(child[0]+1,line)
        else:changed.insert(child[0]+1,f'{child[1]} execve("/bin/true", ["/bin/true"], []) = 0\n')
        changed=''.join(changed).encode();x=copy.deepcopy(raw);x['trace_sha256']=raw_control.sha(changed);bad(lambda:adapt({'O0_meta':enc(x),'O0_trace':changed}),label)
    return count


def fixture_checks():
    scorecard.select_snapshot(authority.REVISION);m,old,_=authority.authority(scorecard);entry=authority.activated_entries(scorecard)['families']['json'];binding=authority.effective_binding(scorecard,entry);frozen=scorecard.load_frozen_scripts();names=scorecard.families();stdlib=scorecard.load_records('stdlib',names,frozen);scripts=scorecard.load_records('script',names,frozen);rows=[r for r in stdlib if r['family']=='json'];fn=[r for r in stdlib if r['family']=='fnmatch'];scorecard.require(len(rows)==30 and all(r['paired'] for r in rows) and len(fn)==24 and all(r['paired'] for r in fn) and len(scripts)==4,'real descendant pairs or predecessor credit differ')
    for group in ('families','script_shards'):
        for e in old[group].values():scorecard.require(authority.safe_path(scorecard,e['receipt']).read_bytes()==raw_control.git(scorecard.ROOT,'show','--no-textconv',authority.ACTIVATION_COMMIT+':'+e['receipt']),'old receipt bytes differ')
    native=json.loads(authority.read(scorecard,entry,binding['roles']['O0_meta']));held=json.loads(authority.read(scorecard,entry,binding['roles']['stock_meta']));scorecard.require(all(r['runs']['O0']['argv']==native['argv'] and r['runs']['O0']['evidence'].startswith(binding['roles']['O0_stderr']+'#L') and r['runs']['stock']['argv']==held['argv'] and r['runs']['stock']['evidence'].startswith(binding['roles']['stock_stderr']+'#L') for r in rows),'credited rows do not join fresh O0/held stock evidence')
    with tempfile.TemporaryDirectory() as tmp:
        dest=Path(tmp)/'candidate';generate.finite(scorecard,binding,dest);scorecard.require(dest.read_bytes()==authority.artifact(scorecard,entry,entry['staged_receipt']).read_bytes(),'generator differs from inactive staged occurrences')
    primary=[scorecard.ROOT/'README.md',scorecard.ROOT/'docs/stdlib.md',scorecard.DATA_ROOT/'README.md',scorecard.DATA_ROOT/'docs/stdlib.md'];family_names={k for k,v in names.items() if v['cohort']=='frozen'};tables=[]
    for path in primary:
        text=path.read_text();section=text[text.index(scorecard.START):text.index(scorecard.END)];table=[line for line in section.splitlines() if line.startswith('| `') and line.split('`')[1] in family_names];scorecard.require(len(table)==45,'primary or revision is missing frozen rows');tables.append(table);scorecard.require('**4 of 84 original command IDs have receipts**' in section and '**80 IDs unobserved at this revision**' in section,'script receipt/unobserved counts differ')
        for target in re.findall(r'\[[^\]]+\]\(([^)]+)\)',section):
            if not target.startswith(('https://','http://','#')):scorecard.require((path.parent/target).resolve().exists(),'generated local link target missing')
    scorecard.require(all(x==tables[0] for x in tables[1:]) and scorecard.root_markers(authority.REVISION)[0] not in primary[0].read_text() and '49f' in primary[0].read_text() and '8571ca2' in primary[0].read_text(),'primary/snapshot row promotion or historical separation differs')
    fs,ss=scorecard.summary();frozen_rows=[v for v in fs.values() if v['cohort']=='frozen'];j=fs['json'];scorecard.require(j['originals']==216 and j['paired']==j['receipts']==30 and all(j['runs'][t]['not_run']==186 for t in scorecard.TIERS) and sum(v['originals'] for v in frozen_rows)==16225 and sum(v['paired'] for v in frozen_rows)==54 and len(ss)==sum(r['paired'] for r in ss)==4 and scorecard.COHORT['scripts']['expected_commands']==84 and '4 verified / 72 historically reported' in scorecard.script_cohort_text(ss).replace('**',''),'activated primary totals differ');n=rebound_checks(m,old,entry,binding,rows,names,frozen)
    print('fresh O0 argv compact-JSON SHA256: '+raw_control.sha(json.dumps(native['argv'],separators=(',',':')).encode()))
    print(f'real descendant: JSON 30/216, 186 not run; frozen stdlib 54/16,225; scripts 4/84, 80 not run, 4/72 historical positives; legacy fnmatch24/scripts4 and predecessor bytes unchanged; four views agree; focused {n} passed, 0 skipped')


def probe():
    counts=closed_environment();manifest=json.loads((HERE/'manifest.json').read_bytes());root=scorecard.ROOT;reject(lambda:authority.fixed_pin(scorecard),'source-stage self-authorization');base=run('git','merge-base','HEAD','main',cwd=root).decode().strip()
    with tempfile.TemporaryDirectory(prefix='successor959-source-',dir='/workspace/scratch') as temp:
        fixture=Path(temp)/'repo';g=lambda *a:run('git','-C',str(fixture),*a);run('git','clone','--quiet','--shared','--no-checkout',str(root),str(fixture));g('symbolic-ref','HEAD','refs/heads/fixture');g('update-ref','refs/heads/fixture',base);g('read-tree',base);g('remote','set-url','origin','https://github.com/astral-experiments/imre.git');g('config','user.name','Charlie Marsh');g('config','user.email','charlie.r.marsh@gmail.com');g('checkout',base,'--','benchmarks/compatibility','benchmarks/cpython_stdlib/full','README.md','docs/stdlib.md');shutil.copytree(HERE,fixture/authority.BASE,dirs_exist_ok=True);g('add','-f','--',authority.BASE);g('commit','--quiet','-m','temporary fixture source stage');stage=g('rev-parse','HEAD').decode().strip();run(sys.executable,'-B','benchmarks/compatibility/scorecard.py','--check',cwd=fixture);run(sys.executable,'-B','benchmarks/compatibility/scorecard.py','--snapshot',authority.REVISION,'--check','--root-summary',cwd=fixture);candidate=[json.loads(x) for x in (fixture/manifest['bindings']['stdlib']['json']['staged_receipt']).read_bytes().splitlines()];scorecard.require(len(candidate)==30 and not any(x['paired'] for x in candidate),'preactivation stage granted credit');raw=(fixture/authority.MANIFEST).read_bytes();pin={'commit':stage,'manifest':authority.MANIFEST,'git_blob':g('rev-parse',stage+':'+authority.MANIFEST).decode().strip(),'sha256':raw_control.sha(raw)}
        for patch in manifest['patches']:
            for item in patch['files']:scorecard.require(raw_control.sha((fixture/item['path']).read_bytes())==item['old_sha256'] and g('hash-object','--',item['path']).decode().strip()==item['old_git_blob'],'preimage differs')
            path=fixture/patch['path'];scorecard.require(raw_control.sha(path.read_bytes())==patch['sha256'],'patch differs');g('apply','--unidiff-zero','--check',str(path));g('apply','--unidiff-zero',str(path))
            for item in patch['files']:scorecard.require(raw_control.sha((fixture/item['path']).read_bytes())==item['post_sha256'],'postimage differs')
        pins=fixture/'benchmarks/compatibility/source_stage_pins.py';pins.write_text(pins.read_text()+'\nREVISION_959_SUCCESSOR = '+repr(pin)+'\n');activation={'schema':'imre.compatibility.successor-activation.v1','source_stage':pin,'families':manifest['bindings']['stdlib'],'receipt_hashes':{}}
        for entry in activation['families'].values():
            rows=[json.loads(x) for x in (fixture/entry['staged_receipt']).read_bytes().splitlines()]
            for row in rows:row['paired']=True;row.pop('credit_reason',None)
            p=fixture/entry['receipt'];p.parent.mkdir(parents=True,exist_ok=True);p.write_text(''.join(json.dumps(x,sort_keys=True,separators=(',',':'))+'\n' for x in rows));activation['receipt_hashes'][entry['receipt']]=raw_control.sha(p.read_bytes())
        ap=fixture/authority.BASE/'activation.json';ap.write_text(json.dumps(activation,sort_keys=True)+'\n');files=[x['path'] for p in manifest['patches'] for x in p['files']]+[str(pins.relative_to(fixture)),str(ap.relative_to(fixture))]+list(activation['receipt_hashes']);g('add','-f','--',*files);g('commit','--quiet','-m','temporary fixture descendant');g('update-ref','refs/remotes/origin/main',g('rev-parse','HEAD').decode().strip());run(sys.executable,'-B','benchmarks/compatibility/scorecard.py',cwd=fixture);run(sys.executable,'-B','benchmarks/compatibility/scorecard.py','--check',cwd=fixture);run(sys.executable,'-B','benchmarks/compatibility/scorecard.py','--snapshot',authority.REVISION,'--check','--root-summary',cwd=fixture);print(run(sys.executable,'-B',str(fixture/authority.BASE/'check_stage.py'),'--fixture',cwd=fixture).decode().strip())
    print(f'closed-environment audit: {counts[0]} files/member views, {counts[1]} environments, {counts[2]} raw execs; 0 violations')

if __name__=='__main__':fixture_checks() if '--fixture' in sys.argv else probe()
