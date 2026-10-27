"""Measure each original control on one standalone runtime-import pair."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

packet, area, oracle = map(Path, sys.argv[1:4])
pair_dir = area/'pair'
pair = json.loads((pair_dir/'pair.json').read_bytes())
manifest = json.loads((packet/'inputs.json').read_bytes())
def sha(p):
    with p.open('rb') as f:
        return hashlib.file_digest(f,'sha256').hexdigest()
def run(cmd, wd, native=False):
    env={'PATH':'/no-host','LC_ALL':'C.UTF-8'} if native else {**os.environ,'LC_ALL':'C.UTF-8'}
    proc=subprocess.run(cmd,cwd=wd,capture_output=True,text=True,env=env,timeout=140)
    return {'exit':proc.returncode,'stdout':proc.stdout.replace(str(wd),'<case>'),'stderr':proc.stderr.replace(str(wd),'<case>')}
def write():
    temp=packet/'controls.tmp';temp.write_text(json.dumps(report,indent=2)+'\n');temp.replace(packet/'controls.json')

assert all(sha(Path(p))==pair['sha256'][n] for n,p in pair['paths'].items())
assert run([oracle,'-ISB','-c','import sys; print(sys.version_info[:3])'],packet)['stdout']=='(3, 14, 5)\n'
report={'source':pair['source'],'pair':pair,'inputs_sha256':sha(packet/'inputs.json'),'python_sha256':sha(oracle),'groups':{},'full_server_runs':0}
imre,archive=(Path(pair['paths'][n]) for n in ('imre','libimre_runtime.a'))
failures=[]
for group,details in manifest['groups'].items():
    original=packet/'inputs'/details['file'];assert sha(original)==details['sha256']
    g=report['groups'][group]={'source_sha256':sha(original),'cpython':{},'native':{}}
    with tempfile.TemporaryDirectory(dir=area/'tmp',prefix=group+'-') as t:
        work=Path(t);app=work/'app';app.mkdir();source=app/'original.py';source.write_bytes(original.read_bytes())
        for case in details['cases']:
            args=[case] if case else [];ref=run([oracle,'-ESB',source,*args],work);assert ref['exit']==0 and not ref['stderr'],(group,case,ref)
            if 'oracle' in details:assert ref==details['oracle'][case],(group,case,ref,details['oracle'][case])
            g['cpython'][case]=ref
        for opt in ('0','3'):
            elf=work/('app-O'+opt);ir=work/('app-O'+opt+'.ll');build=run([imre,'build',source,'--module-root',app,'--runtime',archive,'--runtime-imports','--opt-level',opt,'--clang','/usr/bin/clang-18','--no-cache','--emit-llvm',ir,'-o',elf],work)
            mode=g['native'][opt]={'build':build,'cases':{}}
            if build['exit']:
                failures.append([group,opt,'compile',build['stderr'].splitlines()[-1:]]);write();continue
            lines=ir.read_text().splitlines();calls=[line for line in lines if 'call ' in line]
            mode.update(elf_sha256=sha(elf),llvm_sha256=sha(ir),boxed_entry=any('@imre_dyn_' in l for l in calls),registered=sum('@imre_dyn_register(' in l for l in calls),typed_regex_sub=sum('@imre_regex_sub(' in l for l in calls))
            if details['lowering']=='boxed' and not mode['boxed_entry']:failures.append([group,opt,'missing boxed dispatch'])
            if details['lowering']=='typed_strip' and (mode['boxed_entry'] or not mode['typed_regex_sub']):failures.append([group,opt,'unexpected strip lowering'])
            for case in details['cases']:
                args=[case] if case else [];trace=work/'native.strace'
                rr=run(['/usr/bin/strace','-f','-qq','-s','160','-e','trace=execve,openat,bind,listen','-o',trace,elf,*args],work,True)
                log=trace.read_text();ex=log.count('execve(');python=log.count('libpython');bind=log.count('bind(');listen=log.count('listen(')
                match=rr==g['cpython'][case] and ex==1 and python==0 and bind==0 and listen==0
                item=mode['cases'][case]={'run':rr,'matches':match,'execves':ex,'libpython':python,'binds':bind,'listens':listen}
                if ex!=1 or python or bind or listen:failures.append([group,opt,case,'unexpected process or socket activity'])
                elif not match:
                    previous=details.get('retained_failure',{}).get(case)
                    if previous and (previous in rr['stderr'] or previous=='different stdout' and not rr['exit'] and rr['stdout']!=g['cpython'][case]['stdout']):
                        item['retained_failure']=previous
                    else:failures.append([group,opt,case,(rr['stderr'].splitlines() or ['different stdout'])[-1]])
            print(group,opt,sum(i['matches'] for i in mode['cases'].values()),'/',len(mode['cases']), 'registered',mode['registered'],'typed_sub',mode['typed_regex_sub'],flush=True);write()
report['unexpected_failures']=failures;report['completed_utc']=datetime.datetime.now(datetime.timezone.utc).isoformat();report['terminal']='qualified with measured limits' if not failures else 'unexpected failure';write();print(report['terminal'],failures,flush=True);sys.exit(bool(failures))
