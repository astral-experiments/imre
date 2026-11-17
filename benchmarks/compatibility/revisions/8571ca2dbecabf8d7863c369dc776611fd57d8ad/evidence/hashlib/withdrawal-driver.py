from __future__ import annotations
import collections,gzip,hashlib,json,os,pathlib,re,signal,subprocess,sys,time
D=pathlib.Path('/workspace/imre-857-data');C=D/'benchmarks/compatibility';REV='8571ca2dbecabf8d7863c369dc776611fd57d8ad';R=C/'revisions'/REV;E=R/'evidence/hashlib';S=pathlib.Path('/workspace/scratch/hashlib-857');FIX=pathlib.Path('/workspace/scratch/imre-cpython-test-fixtures/Lib');CPY=pathlib.Path('/workspace/scratch/cpython-5607950ef232dad16d75c0cf53101d9649d89115/Lib');IMRE=pathlib.Path('/workspace/imre/target/release/imre');RT=IMRE.parent/'libimre_runtime.a';CLANG=pathlib.Path('/workspace/.toolchains/imre-x86_64-unknown-linux-gnu/imre-toolchain/bin/clang');sys.path.insert(0,str(C));from scorecard import parse_result

def h(p):
 x=hashlib.sha256()
 with p.open('rb') as f:
  for b in iter(lambda:f.read(1048576),b''):x.update(b)
 return x.hexdigest()
def run(argv,env,timeout,stdout,stderr):
 started=time.monotonic()
 with stdout.open('wb') as o,stderr.open('wb') as e:
  p=subprocess.Popen(list(map(str,argv)),cwd=D,env=env,stdout=o,stderr=e,start_new_session=True)
  try:code=p.wait(timeout=timeout);timed=False
  except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);p.wait();code=None;timed=True
 return {'argv':list(map(str,argv)),'exit_code':code,'timed_out':timed,'timeout_seconds':timeout,'elapsed_seconds':round(time.monotonic()-started,3),'stdout_sha256':h(stdout),'stderr_sha256':h(stderr)}
def dump(path,obj):path.write_text(json.dumps(obj,indent=2)+'\n')
gate=json.loads((E/'control/run.json').read_text());assert gate['gate_passed'];b=json.loads((E/'build.json').read_text());assert h(IMRE)==b['cli']['sha256'] and h(RT)==b['runtime']['sha256'];rows=[json.loads(x) for x in (R/'stdlib/hashlib.jsonl').read_text().splitlines()];keys={(r['id'],r['occurrence']) for r in rows};assert len(keys)==82
base=os.environ.copy();base.pop('IMRE_ENCODED_FLAGS',None);base.update(PATH=str(CLANG.parent)+os.pathsep+base['PATH'],PYTHONPATH=str(FIX),PYTHONDONTWRITEBYTECODE='1',IMRE_PYTHON=str(IMRE),PYTHON_COLORS='0',NO_COLOR='1');meta={'revision':REV,'resource_policy':'test.support.use_resources = []','gate':str((E/'control/run.json').relative_to(D)),'runs':{}};images={}
for tier,opt,limit in (('O0','0',300),('O3','3',600)):
 image=S/f'{tier}.exe';assert not image.exists();env=base.copy();env['IMREFLAGS']='--opt-level '+opt;cmd=[IMRE,'build',S/'positive.py','--module-root',S,'--no-cache','--opt-level',opt,'--runtime',RT,'--clang',CLANG,'-o',image,'--extension-path',FIX,'--include-module','test.test_hashlib'];br=run(cmd,env,limit,E/f'{tier}.build.stdout',E/f'{tier}.build.stderr');info={'build':br,'environment':{k:env[k] for k in ('PATH','PYTHONPATH','PYTHONDONTWRITEBYTECODE','IMREFLAGS','IMRE_PYTHON','PYTHON_COLORS','NO_COLOR')},'image_exists':image.exists()}
 if br['exit_code']==0 and image.exists():info['image_sha256']=h(image);images[tier]=image
 else:info['reason']='native build timed out before test execution' if br['timed_out'] else 'native build failed before test execution'
 meta['runs'][tier]=info;print(tier,'build',br['exit_code'],'image',image.exists(),flush=True)
withdraw={'sources':[],'bytecode_before':[],'bytecode_absent_during':None,'runs':{},'restored':False};custody=pathlib.Path('/workspace/scratch/hashlib-857-custody');custody.mkdir(exist_ok=True);sources=[FIX/'test/test_hashlib.py',CPY/'test/test_hashlib.py',S/'positive.py'];bytecode=[]
for root,stem in ((FIX/'test','test_hashlib'),(CPY/'test','test_hashlib'),(S,'positive')):
 bytecode += [p for p in (root/f'{stem}.pyc',root/f'{stem}.pyo') if p.exists()]
 if (root/'__pycache__').exists():bytecode += list((root/'__pycache__').glob(stem+'.*.pyc'))
withdraw['bytecode_before']=[str(p) for p in bytecode];moved=[]
if images:
 try:
  for n,p in enumerate(sources+bytecode):
   assert p.is_file();dest=custody/f'{n}-{p.name}';assert not dest.exists();digest=h(p);p.rename(dest);moved.append((p,dest,digest));withdraw['sources'].append({'path':str(p),'sha256_before':digest,'custody':str(dest),'kind':'bytecode' if p in bytecode else 'source'})
  assert all(not p.exists() for p,_,_ in moved);withdraw['bytecode_absent_during']=all(not p.exists() for p in bytecode) and all(not list((root/'__pycache__').glob(stem+'.*.pyc')) if (root/'__pycache__').exists() else True for root,stem in ((FIX/'test','test_hashlib'),(CPY/'test','test_hashlib'),(S,'positive')))
  for tier,image in images.items():
   info=meta['runs'][tier];env=base.copy();env['IMREFLAGS']=info['environment']['IMREFLAGS'];control={};
   for name,cmd in (('readelf',['readelf','-d',str(image)]),('ldd',['ldd',str(image)])):
    raw=run(cmd,env,30,E/f'{tier}.{name}.stdout',E/f'{tier}.{name}.stderr');control[name]=raw
   trace=E/f'{tier}.exec-open.trace';argv=['strace','-f','-e','trace=execve,openat','-o',str(trace),str(image),'test.test_hashlib'];rr=run(argv,env,240,E/f'{tier}.stdout',E/f'{tier}.stderr');trace_text=trace.read_text(errors='replace');exec_lines=[l for l in trace_text.splitlines() if re.search(r'\bexecve\(',l)];deps=(E/f'{tier}.readelf.stdout').read_text(errors='replace')+(E/f'{tier}.ldd.stdout').read_text(errors='replace');control.update({'sources_absent':all(not p.exists() for p,_,_ in moved),'execve_count':len(exec_lines),'initial_exec_matches_image':len(exec_lines)==1 and str(image) in exec_lines[0],'child_exec_observed':len(exec_lines)!=1 or not(exec_lines and str(image) in exec_lines[0]),'libpython_observed':bool(re.search('libpython',trace_text+deps,re.I)),'trace_sha256':h(trace),'trace_bytes':trace.stat().st_size,'trace_path':str(trace.relative_to(D))});control['passed']=control['sources_absent'] and withdraw['bytecode_absent_during'] and control['initial_exec_matches_image'] and not control['libpython_observed'] and all(control[x]['exit_code']==0 for x in ('readelf','ldd'))
   if trace.stat().st_size>1000000:
    gz=trace.with_suffix(trace.suffix+'.gz')
    with trace.open('rb') as src,gzip.open(gz,'wb') as dst:
     while chunk:=src.read(1048576):dst.write(chunk)
    trace.unlink();control['trace_path']=str(gz.relative_to(D));control['archive_sha256']=h(gz)
   parsed=[];counts=collections.Counter();lines=(E/f'{tier}.stderr').read_text(errors='replace').splitlines()
   for line_no,line in enumerate(lines,1):
    result=parse_result(line,'test.test_hashlib')
    if result:
     ident,status=result;counts[ident]+=1;key=(ident,counts[ident]);assert key in keys,key;parsed.append((key,status,line_no))
   rr['parsed_originals']=len(parsed);rr['summary_totals']=re.findall(r'^Ran (\d+) tests? in ', '\n'.join(lines),re.M);info.update(execution=rr,controls=control,observations=parsed);withdraw['runs'][tier]=control;print(tier,'run',rr['exit_code'],'exact results',len(parsed),'controls',control['passed'],flush=True)
 finally:
  for p,dest,digest in reversed(moved):assert not p.exists();dest.rename(p);assert h(p)==digest
  withdraw['restored']=all(p.exists() and h(p)==digest for p,_,digest in moved)
  for x in withdraw['sources']:x['sha256_after']=h(pathlib.Path(x['path']));x['restored']=x['sha256_after']==x['sha256_before']
else:withdraw['not_run_reason']='neither native build produced an image'
assert withdraw['restored'] or not images;dump(E/'withdrawal.json',withdraw);rel=E.relative_to(D).as_posix()
for tier in ('O0','O3'):
 info=meta['runs'][tier];obs={tuple(key):(status,line) for key,status,line in info.pop('observations',[])};image=S/f'{tier}.exe';
 for row in rows:
  key=(row['id'],row['occurrence']);found=obs.get(key);nr={'argv':[str(image),'test.test_hashlib'],'build_argv':info['build']['argv'],'environment':info['environment']}
  if found:nr.update(outcome=found[0],evidence=f'{rel}/{tier}.stderr#L{found[1]}')
  else:nr.update(outcome='not_run',reason=info.get('reason','suite timed out or exited before this original produced a result'),evidence=f'{rel}/{tier}.build.stderr' if 'execution' not in info else f'{rel}/{tier}.stderr')
  row['runs'][tier]=nr
for row in rows:
 observed=all(row['runs'][t]['outcome']=='pass' for t in ('stock','O0','O3'));controls=gate['gate_passed'] and withdraw['restored'] and all(meta['runs'][t].get('controls',{}).get('passed') for t in ('O0','O3'));row['paired']=bool(observed and controls);row['omission_reason']=None if not observed or controls else 'native withdrawal, no-child-exec, or no-libpython control failed; see revision evidence'
(R/'stdlib/hashlib.jsonl').write_text(''.join(json.dumps(r,sort_keys=True)+'\n' for r in rows));dump(E/'run.json',meta);print('paired',sum(r['paired'] for r in rows),flush=True)
