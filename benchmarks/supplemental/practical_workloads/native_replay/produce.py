"""Capture four frozen practical workloads at an explicitly authorized public native revision."""
from __future__ import annotations
import argparse, http.server, json, os, shutil, signal, subprocess, sys, threading, time
from pathlib import Path
import validate_native as v

STOCK=v.HERE.parent/'stock_v1';sys.path.insert(0,str(STOCK));import capture_stock as frozen
PROBE=v.COMPAT/'revisions/8571ca2dbecabf8d7863c369dc776611fd57d8ad/controls/raw_control_probe.c'
def write(p,x): p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(x,indent=2,sort_keys=True)+'\n')
def expand(x,roles):
    for k,val in sorted(roles.items(),key=lambda i:-len(i[0])):x=x.replace(k,val)
    return x
def capture(argv,cwd,env,timeout):
    with subprocess.Popen(argv,cwd=cwd,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True) as p:
        try:so,se=p.communicate(timeout=timeout);return p.returncode,so,se,False
        except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);so,se=p.communicate();return None,so,se,True
def traced(syscalls,path,args):return ['/usr/bin/strace','-f','-qq','-s','512','-e','trace='+syscalls,'-o',str(path),*map(str,args)]
def run_files(o,prefix,argv,cwd,env,timeout):
    code,so,se,timed=capture(argv,cwd,env,timeout);(o/(prefix+'stdout')).write_bytes(so);(o/(prefix+'stderr')).write_bytes(se);return {'argv':argv,'exit_code':code,'timed_out':timed,'stdout_sha256':v.raw.sha(so),'stderr_sha256':v.raw.sha(se)}
def command(identity,key,argv,commands):
    p=subprocess.run(argv,capture_output=True,check=True);name=key+'.raw';(identity/name).write_bytes(p.stdout);commands[key]={'argv':argv,'stdout':name,'sha256':v.raw.sha(p.stdout),'exit_code':p.returncode}
def preimage(work,ext,case,src,ref):
    work.mkdir();(work/'scripts').mkdir(mode=0o755);(work/'home').mkdir(mode=0o755);entry=work/src['path'];shutil.copyfile(STOCK/src['vendored'],entry);entry.chmod(0o644);ext.mkdir();(ext/'fixtures').mkdir()
    if case=='ty-readme':
        for x in v.load(STOCK/'ty-inputs.json'):p=work/x['path'];shutil.copyfile(STOCK/x['vendored'],p);p.chmod(0o644)
    for x in ref['fixture']['external_inputs']:
        if x['path'].startswith('$CORPUS/'):
            relative=x['path'].removeprefix('$CORPUS/');p=ext/relative;shutil.copyfile(STOCK/relative,p);v.need(v.sha(p)==x['sha256'],'frozen external input')
    return entry
def build_argv(b,entry,work,t,image,archive):return [b['compiler'],'build',str(entry),'--module-root',str(work),'--no-cache','--opt-level',t[1],'--runtime',b['runtime'],'--clang',b['clang'],*(['--extension-path',str(archive)] if archive else []),'-o',str(image)]
def poison_gate(o,entry,work,positive,b,env):
    original=entry.read_bytes();marker=b'from __future__ import annotations\n'
    v.need(original.count(marker)==1,'frozen launcher gate insertion');injected=original.replace(marker,marker+b'import architecture_control\n',1)
    module=work/'architecture_control.py';entry.write_bytes(injected)
    module.write_bytes(b'status: int = True\n');(o/'gate.launcher.py.txt').write_bytes(injected)
    (o/'gate.module.py.txt').write_bytes(module.read_bytes());image=Path(positive[-1]+'.gate')
    argv=positive.copy();argv[-1]=str(image)
    trace=o/'gate.trace';cmd=traced(v.GATE_SYSCALLS,trace,[b['probe'],'gate',image,*argv])
    try:r=run_files(o,'gate.',cmd,work,env,90)
    finally:entry.write_bytes(original);entry.chmod(0o644);module.unlink()
    calls=v.raw.execs(trace.read_text().splitlines(),v.need)
    m=v.re.search(rf'^{calls[1][1]}\s+exit_group\((\d+)\)',trace.read_text(),v.re.M)
    v.need(m is not None,'gate child exit missing')
    g={'argv':cmd,'build_argv':argv,'image':str(image),'positive_sha256':v.raw.sha(original),'injected_sha256':v.raw.sha(injected),'module_sha256':v.sha(o/'gate.module.py.txt'),'stdout_sha256':r['stdout_sha256'],'stderr_sha256':r['stderr_sha256'],'trace_sha256':v.sha(trace),'probe_exit_code':r['exit_code'],'compiler_exit_code':int(m[1]),'image_absent_after':not image.exists()}
    write(o/'gate.json',g)
    v.need(not r['timed_out'] and r['exit_code']==0 and not image.exists(),'actual launcher gate failed');v.raw.trace_gate(trace.read_text(),argv,b['probe'],str(module),(o/'gate.stderr').read_text(),v.need);return g

def tier(out,scratch,b,case,t,src,ref,env,artifacts,reviewed):
    o=out/'evidence'/case/t;o.mkdir(parents=True);work=scratch/(case+'-'+t);ext=scratch/(case+'-'+t+'-external');entry=preimage(work,ext,case,src,ref);before=frozen.inventory(work);v.need(before==v.load(STOCK/'references'/case/'raw/before.json'),'preimage differs');write(o/'before.json',before)
    roles={'$WORK':str(work),'$CORPUS':str(ext),'$PYTHON':b['python_fixture']};external=[ext/x['path'].removeprefix('$CORPUS/') for x in ref['fixture']['external_inputs'] if x['path'].lower().endswith(('.zip','.whl','.pyz'))]
    v.need(len(external)<=1,'archive inventory');archive=external[0] if external else None
    image=scratch/(case+'-'+t+'.exe');argv=build_argv(b,entry,work,t,str(image),archive)
    r={'case':case,'tier':t,'source':src,'reference_sha256':v.sha(STOCK/'references'/case/'reference.json')}
    if t=='O0':
        try:r['gate']=poison_gate(o,entry,work,argv,b,env);v.gate(o,{'build':{'argv':argv},'gate':r['gate']},STOCK/src['vendored'],b,b['probe'])
        except (ValueError,OSError) as e:r.update(status='blocked_gate',reason=str(e));write(o/'receipt.json',r);return False
    started=time.monotonic();r['build']=run_files(o,'build.',argv,work,env,180);r['build'].update(elapsed_seconds=round(time.monotonic()-started,2),image_exists=image.exists(),image_sha256=v.sha(image) if image.exists() else None)
    if r['build']['exit_code']!=0 or not image.exists():r['status']='build_blocker';write(o/'receipt.json',r);return False
    artifacts.append(str(image))
    r['linkage']={}
    for name,command0 in [('readelf',['readelf','-d',str(image)]),('ldd',['ldd',str(image)])]:r['linkage'][name]=run_files(o,name+'.',command0,work,env,10)
    publisher=case in v.FIXTURE_CASES;server=thread=None;frozen.Registry.events=[]
    if publisher:
        server=http.server.ThreadingHTTPServer(('127.0.0.1',0),frozen.Registry);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();authority=f'127.0.0.1:{server.server_port}';roles.update({'$REGISTRY_ORIGIN':'http://'+authority,'$REGISTRY_AUTHORITY':authority})
    app=[expand(x,roles) for x in ref['invocation']['argv'][3:]];runenv={k:expand(x,roles) for k,x in ref['invocation']['env'].items()};stash=scratch/(case+'-'+t+'.source');zstash=scratch/(case+'-'+t+'.zip');trace=o/'trace';cmd=traced(v.RUN_SYSCALLS,trace,[b['supervisor'],entry,stash,archive or '-',zstash if archive else '-',image,src['path'],*app]);stub=ext/'fixtures/cargo_stub.py';stdlib=Path(b['python_fixture']).parent.parent/'lib/python3.14';witness=['sha256sum',b['python_fixture'],str(stub),str(entry),str(image),*[str(stdlib/x) for x in reviewed['stdlib']]]
    if publisher:(o/'fixture-before.sha256').write_bytes(subprocess.check_output(witness));artifacts.append(str(stub))
    v.need(not list(work.rglob('*.pyc')) and not list(work.rglob('*.pyo')),'bytecode preimage')
    try:run=run_files(o,'',cmd,work,runenv,30)
    finally:
        if server:server.shutdown();server.server_close();thread.join()
        if stash.exists():shutil.move(stash,entry)
        if archive and zstash.exists():shutil.move(zstash,archive)
    if run['timed_out']:r.update(run=run,status='timeout');write(o/'receipt.json',r);return True
    if publisher:(o/'fixture-after.sha256').write_bytes(subprocess.check_output(witness))
    after=frozen.inventory(work)
    write(o/'after.json',after)
    processes=(work/'cargo.log').read_bytes() if (work/'cargo.log').exists() else b''
    events=frozen.Registry.events
    (o/'processes.jsonl').write_bytes(processes)
    (o/'registry.jsonl').write_text(''.join(json.dumps(x,sort_keys=True)+'\n' for x in events))
    a0={x['path']:x for x in before}
    a1={x['path']:x for x in after}
    changes={'created':[x for k,x in a1.items() if k not in a0],'deleted':[x for k,x in a0.items() if k not in a1],'modified':[x for k,x in a1.items() if k in a0 and x!=a0[k]]}
    effects=frozen.normalized_effects({'files':changes,'registry':events,'processes':[json.loads(x) for x in processes.splitlines()]},roles)
    write(o/'effects.normalized.json',effects)
    refdir=STOCK/'references'/case
    expected=v.load(refdir/'effects.normalized.json')
    dims={'status':run['exit_code']==ref['observation']['exit_code'] and not run['timed_out'],'stdout':(o/'stdout').read_bytes()==(refdir/'raw/stdout').read_bytes(),'stderr':(o/'stderr').read_bytes()==(refdir/'raw/stderr').read_bytes(),'mutations':effects['files']==expected['files'],'http_requests':effects['registry']==expected['registry'],'cargo_journal':effects['processes']==expected['processes']}
    run.update(cwd=str(work),roles=roles,environment=runenv,workload_argv=[src['path'],*app],umask='0077',source_restored_sha256=v.sha(entry),zip_restored_sha256=v.sha(archive) if archive else None,bytecode_present_before=False,trace_sha256=v.sha(trace),comparison_dimensions=dims)
    r.update(run=run,status='observed_exact' if all(dims.values()) else 'observed_mismatch')
    write(o/'receipt.json',r)
    return True

def main():
    p=argparse.ArgumentParser();p.add_argument('--source',type=Path,required=True)
    p.add_argument('--revision',required=True);p.add_argument('--output',type=Path,required=True)
    p.add_argument('--scratch',type=Path,required=True);p.add_argument('--target',type=Path,required=True)
    p.add_argument('--python',type=Path,required=True);p.add_argument('--clang',type=Path,required=True);p.add_argument('--cargo',type=Path,required=True);p.add_argument('--rustc',type=Path,required=True)
    p.add_argument('--policy-commit',required=True);p.add_argument('--policy-path',required=True)
    a=p.parse_args();a.source=a.source.resolve()
    a.output=a.output.resolve();a.scratch=a.scratch.resolve()
    a.target=a.target.resolve();v.need(not a.output.exists() and not a.scratch.exists() and not any(x.is_relative_to(a.source) for x in (a.output,a.scratch,a.target)),'output/scratch/target must be outside clean source')
    v.need(v.re.fullmatch(r'[0-9a-f]{40}',a.revision),'exact revision');a.cargo=a.cargo.resolve(strict=True);a.rustc=a.rustc.resolve(strict=True);a.clang=a.clang.resolve(strict=True);subprocess.run(['git','-C',str(a.source),'merge-base','--is-ancestor',a.revision,'refs/remotes/origin/main'],check=True)
    sys.path.insert(0,str(STOCK));import validate as stock_validator
    stock_validator.main();repo=STOCK.parents[3];policy_spec=a.policy_commit+':'+a.policy_path;binding={'commit':a.policy_commit,'path':a.policy_path,'blob':v.git(repo,'rev-parse',policy_spec).decode().strip(),'sha256':v.raw.sha(v.git(repo,'show',policy_spec))};reviewed=v.policy_blob(repo,binding);subprocess.run(['git','-C',str(STOCK.parents[3]),'diff','--quiet',v.PIN,'--',str(STOCK.relative_to(STOCK.parents[3]))],check=True)
    os.umask(0o077);a.output.mkdir(parents=True)
    a.scratch.mkdir(parents=True);identity=a.output/'evidence/identity'
    identity.mkdir(parents=True);commands={'prebuild_order':['head','tree','status','cargo_build'],'manifest_path':str(identity/'commands.json')}
    cargo_cc_invocation='/usr/bin/cc';cargo_cc=Path(cargo_cc_invocation).resolve(strict=True);tmpdir=a.scratch/'tmp';tmpdir.mkdir();env={'CARGO_HOME':'/workspace/.toolchains/cargo','RUSTUP_HOME':'/workspace/.toolchains/rustup','PATH':'/usr/bin:/bin','HOME':'/workspace','RUSTC':str(a.rustc),'CC':cargo_cc_invocation,'TMPDIR':str(tmpdir)};probe=a.scratch/'raw_control_probe';sup=a.scratch/'supervisor'
    for key,dst,src in [('probe_build',probe,PROBE),('supervisor_build',sup,v.HERE/'supervisor.c')]:command(identity,key,['/usr/bin/cc','-O2','-Wall','-Wextra','-o',str(dst),str(src)],commands)
    for key,path in [('cargo',a.cargo),('rustc',a.rustc),('clang',a.clang)]:
        command(identity,key+'_version',[str(path),'--version'],commands);v.need(v.re.search(rf'\b{key}\b', (identity/(key+'_version.raw')).read_text(),v.re.I), 'effective tool is not '+key)
    command(identity,'cargo_cc_effective',['/usr/bin/readlink','-f',cargo_cc_invocation],commands);v.need((identity/'cargo_cc_effective.raw').read_text()==str(cargo_cc)+'\n','effective Cargo C compiler differs');command(identity,'cargo_cc_version',[cargo_cc_invocation,'--version'],commands)
    private_python=a.python.resolve(strict=True);command(identity,'python_identity',[str(private_python),'-I','-c',v.PY_IDENTITY],commands);private_identity=json.loads((identity/'python_identity.raw').read_bytes());v.need(private_identity['executable']==str(private_python),'effective private executable differs');private_root=v.fixture_root(str(private_python),private_identity['prefix'],reviewed['root_relative'])
    for key,args in [('head',['rev-parse','HEAD']),('tree',['rev-parse','HEAD^{tree}']),('status',['status','--porcelain','--untracked-files=all'])]:command(identity,key,['git','-C',str(a.source),*args],commands)
    v.need((identity/'head.raw').read_text()==a.revision+'\n' and (identity/'status.raw').read_bytes()==b'','source must be exact clean public revision')
    cargo=[str(a.cargo),'build','--offline','--release','--locked','-vv','-p','imre-compiler','-p','imre-runtime','--target-dir',str(a.target)]
    code,so,se,timed=capture(cargo,a.source,env,900)
    (a.output/'build.log').write_bytes(so+se)
    v.need(code==0 and not timed,'release build failed')
    cli=a.target/'release/imre'
    runtime=a.target/'release/libimre_runtime.a'
    b={'pin':a.revision,'cargo_cc_invocation':cargo_cc_invocation,'tree':(identity/'tree.raw').read_text().strip(),'manifest_path':str(a.output/'evidence/build.json'),'cargo_build':{'cwd':str(a.source),'argv':cargo,'exit_code':code,'log_sha256':v.sha(a.output/'build.log'),'environment':env},'supervisor_source':str(v.HERE/'supervisor.c'),'supervisor_source_sha256':v.sha(v.HERE/'supervisor.c'),'probe_source':str(PROBE),'stub_sha256':v.sha(STOCK/'fixtures/cargo_stub.py'),'fixture_policy':binding,'python_prefix':private_identity['prefix'],'stdlib_root':str(private_root),'tmpdir':str(tmpdir),'depfiles':{}}
    for key,artifact in [('compiler',cli),('runtime',runtime)]:
        dep=artifact.with_suffix('.d');v.need(dep.is_file(),'missing '+key+' depfile; abort and let the coordinator choose an isolated target after a disk check');payload=dep.read_bytes();(identity/(key+'.d')).write_bytes(payload);b['depfiles'][key]={'path':str(dep),'sha256':v.raw.sha(payload)}
    v.prebuild_provenance({**b,'compiler':str(cli),'runtime':str(runtime)},(a.output/'build.log').read_text(),(identity/'compiler.d').read_bytes(),(identity/'runtime.d').read_bytes())
    for key,path in [('compiler',cli),('runtime',runtime),('cargo',a.cargo),('rustc',a.rustc),('clang',a.clang),('cargo_cc',cargo_cc),('python_fixture',a.python),('supervisor',sup),('probe',probe),('producer',Path(__file__).resolve()),('validator',Path(v.__file__).resolve())]:b[key]=str(path.resolve());b[key+'_sha256']=v.sha(path)
    v.need(b['python_fixture_sha256']==v.load(STOCK/'references/ruff-publisher/reference.json')['tool']['executable_sha256'],'fixture interpreter mismatch');write(a.output/'evidence/build.json',b);artifacts=[b[k] for k in ('compiler','runtime','cargo','rustc','clang','cargo_cc','python_fixture','supervisor','probe','producer','validator','supervisor_source','probe_source')]+[b['manifest_path'],commands['manifest_path']]
    stdlib=Path(b['python_fixture']).parent.parent/'lib/python3.14';v.need(reviewed['stub_sha256']==b['stub_sha256'] and all(v.sha(stdlib/rel)==digest for rel,digest in reviewed['stdlib'].items()),'reviewed fixture policy bytes differ');artifacts += [str(stdlib/rel) for rel in reviewed['stdlib']]
    for case,src in v.load(STOCK/'sources.json').items():
        ref=v.load(STOCK/'references'/case/'reference.json');ok=tier(a.output,a.scratch,b,case,'O0',src,ref,env,artifacts,reviewed)
        if ok:tier(a.output,a.scratch,b,case,'O3',src,ref,env,artifacts,reviewed)
        else:o=a.output/'evidence'/case/'O3';o.mkdir();write(o/'receipt.json',{'case':case,'tier':'O3','status':'not_run','reason':'O0 gate/build blocked'})
    write(identity/'commands.json',commands);(identity/'artifacts.sha256').write_bytes(subprocess.check_output(['sha256sum',*artifacts]));print('review artifact SHA256:',v.sha(identity/'artifacts.sha256'))

if __name__=='__main__':main()
