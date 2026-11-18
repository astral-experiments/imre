"""Bounded receiver-side original-anchor replay using transferred donor-built 95908 products."""
from pathlib import Path
import ast,hashlib,json,os,re,signal,subprocess,sys,time
base=Path('/workspace/uv-b-959-replay-v1');base.mkdir();git=Path('/workspace/uv-b-recovery/uv.git');uv='24688aaca1e7fd104437a0909d5db3afdc7dcdb0';product=Path('/workspace/scratch/imre-95908-transferred-v1/product');cli=product/'imre';runtime=product/'libimre_runtime.a';clang=product/'imre-toolchain/bin/clang';py=Path('/workspace/.toolchains/uv-python/cpython-3.14.5-linux-x86_64-gnu/bin/python3.14');supervisor=Path('/workspace/practical-d26-scratch-retry/supervisor');compat=Path('/workspace/imre-practical-publish/benchmarks/compatibility');sys.path.insert(0,str(compat));import scorecard
sha=lambda d:hashlib.sha256(d).hexdigest();file_sha=lambda p:hashlib.file_digest(Path(p).open('rb'),'sha256').hexdigest();assert file_sha(cli)=='e2d0886948f766332df6370a2e3140a5be5c8b0fc108224ef7242fc38fc58bd4' and file_sha(runtime)=='33ae5dfef5eff3c1fac5e0a5e19263daa980d7d80a5917979b426e85fa128c5c' and file_sha(clang)=='8ef402d453d1ba4902e4ee0f0f847f6cfa01400c95aa43c24e97818b9c0e3f45';provenance=json.loads((product.parent/'transfer-provenance.json').read_text());assert provenance['source_revision']=='95908fb0acadc2a67304a8080e584deaef395746' and provenance['source_tree']=='e6405a3243de706adcab772bf16942808d6ca6fb';anchors={x['id']:x for x in (json.loads(z) for z in (compat/'frozen/scripts.jsonl').read_text().splitlines()) if x['id'] in ('uv-26','uv-29','uv-extra-02')};tracecalls='execve,execveat,open,openat,openat2,creat,io_uring_setup,clone,clone3,fork,vfork,rename,renameat,renameat2,newfstatat,wait4,exit_group,chdir,fchdir,close,dup,dup2,dup3,fcntl';summary={'custody':'donor-built transferred binary; no receiver build','revision':provenance['source_revision'],'tree':provenance['source_tree'],'archive_sha256_metadata_only':provenance['sha256'],'compiler_sha256':file_sha(cli),'runtime_sha256':file_sha(runtime),'clang_sha256':file_sha(clang),'python_sha256':file_sha(py),'supervisor_sha256':file_sha(supervisor),'cases':{}}
def write(p,x):p.write_text(json.dumps(x,indent=2,sort_keys=True)+'\n')
def capture(argv,cwd,env,seconds,o,prefix):
 start=time.monotonic();p=subprocess.Popen(argv,cwd=cwd,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)
 try:so,se=p.communicate(timeout=seconds);timed=False
 except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);so,se=p.communicate();timed=True
 (o/(prefix+'stdout')).write_bytes(so);(o/(prefix+'stderr')).write_bytes(se);return {'argv':list(map(str,argv)),'cwd':str(cwd),'environment':env,'exit_code':p.returncode,'timed_out':timed,'elapsed_seconds':round(time.monotonic()-start,2),'stdout_sha256':sha(so),'stderr_sha256':sha(se)}
def fixture(id,role):
 a=anchors[id];raw=json.loads(a['evidence']['raw']);o=base/id/role;o.mkdir(parents=True);w=o/'work';h=o/'home';w.mkdir();h.mkdir();before={}
 for path,digest in raw['preimages'].items():
  d=subprocess.check_output(['git','--git-dir='+str(git),'show',uv+':'+path]);assert sha(d)==digest;p=w/path;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(d);before[path]={'bytes':len(d),'sha256':sha(d)}
 roles={'$PYTHON':str(py),'$WORK':str(w),'$TASK_HOME':str(h)}
 def expand(s):
  for k,v in roles.items():s=s.replace(k,v)
  return s
 inv=a['normalized_invocation'];env={k:expand(v) for k,v in inv['env'].items()};argv=[expand(x) for x in inv['argv']];return a,raw,o,w,env,argv,before
def compare(raw,o,before,run,w):
 effects={}
 for path,pre in before.items():
  p=w/path;post={'bytes':p.stat().st_size,'sha256':file_sha(p)} if p.is_file() else None
  if pre!=post:effects[path]=[pre,post]
 text=json.dumps(effects,separators=(',',':'),sort_keys=True);(o/'effects.json').write_text(text);ref=raw['reference'];dims={'exit':run['exit_code']==ref['exit_code'] and not run['timed_out'],'stdout':(o/'run.stdout').read_bytes()==ref['stdout'].encode(),'stderr':(o/'run.stderr').read_bytes()==ref['stderr'].encode(),'effects':text==ref['effects_manifest'] and sha(text.encode())==ref['effects_sha256']};return {'dimensions':dims,'effects_sha256':sha(text.encode()),'exact':all(dims.values())}
def buildcmd(entry,w,opt,image):return [str(cli),'build',str(entry),'--module-root',str(w),'--no-cache','--opt-level',str(opt),'--runtime',str(runtime),'--clang',str(clang),'-o',str(image)]
def direct_gate(id):
 o=base/id/'direct-control';o.mkdir();src=o/'direct.py';src.write_bytes(b'status: int = True\n');env=summary['cases'][id]['stock']['environment'];image=o/'direct.exe';r=capture(buildcmd(src,o,0,image),o,env,90,o,'');r.update(image_exists=image.exists(),source_sha256=file_sha(src),diagnostic=(o/'stderr').read_text(errors='replace'));r['rejected']=r['exit_code']!=0 and not image.exists() and 'expected int, got bool' in r['diagnostic'];write(o/'receipt.json',r);return r
def imported_gate(id):
 a,raw,o,w,env,argv,before=fixture(id,'import-control');entry=w/a['source']['path'];original=entry.read_bytes();tree=ast.parse(original);body=tree.body;at=0
 if body and isinstance(body[0],ast.Expr) and isinstance(body[0].value,ast.Constant) and isinstance(body[0].value.value,str):at=body[0].end_lineno
 for node in body:
  if isinstance(node,ast.ImportFrom) and node.module=='__future__':at=max(at,node.end_lineno)
 lines=original.splitlines(keepends=True);injected=b''.join(lines[:at])+b'\nimport architecture_control\n'+b''.join(lines[at:]);entry.write_bytes(injected);poison=w/'architecture_control.py';poison.write_bytes(b'status: int = True\n');image=o/'negative.exe';r=capture(buildcmd(entry,w,0,image),w,env,90,o,'');r.update(original_source_sha256=sha(original),injected_source_sha256=sha(injected),poison_sha256=file_sha(poison),image_exists=image.exists(),diagnostic=(o/'stderr').read_text(errors='replace'));r['rejected']=r['exit_code']!=0 and not image.exists() and str(poison) in r['diagnostic'] and 'expected int, got bool' in r['diagnostic'];write(o/'receipt.json',r);return r
def native(id,tier):
 a,raw,o,w,env,argv,before=fixture(id,tier);entry=w/a['source']['path'];image=o/'saved.exe';build=capture(buildcmd(entry,w,int(tier[1]),image),w,env,180,o,'build.');build.update(image_exists=image.exists(),image_sha256=file_sha(image) if image.exists() else None);row={'build':build,'original_normalized_invocation':a['normalized_invocation']}
 if build['exit_code']!=0 or not image.exists():row['status']='build_blocker';write(o/'receipt.json',row);return row
 trace=o/'trace';stash=o/'withdrawn.source';sargv=[str(supervisor),str(entry),str(stash),'-','-',str(image),a['source']['path'],*argv[3:]];cmd=['/usr/bin/strace','-f','-qq','-s','512','-e','trace='+tracecalls,'-o',str(trace),*sargv];run=capture(cmd,w,env,30,o,'run.');assert entry.exists() and file_sha(entry)==a['source']['sha256'];comp=compare(raw,o,before,run,w);text=trace.read_text();exec_lines=[s for s in text.splitlines() if re.search(r'\bexecve(?:at)?\(',s)];opens=list(scorecard.successful_open_paths(text,str(w)));forbidden=[str(p) for p in opens if p.suffix.lower() in ('.py','.pyc','.pyo','.zip','.whl','.pyz') or 'libpython' in str(p).lower()];changes=[s for s in text.splitlines() if re.search(r'\b(?:chdir|fchdir|execveat|io_uring_setup)\(',s) and re.search(r'= (?:0|[1-9][0-9]*)$',s)];withdraw=any('rename('+json.dumps(str(entry))+', '+json.dumps(str(stash))+') = 0' in s for s in text.splitlines()) and any(json.dumps(str(entry)) in s and 'ENOENT' in s for s in text.splitlines());row.update(run=run,comparison=comp,trace={'sha256':file_sha(trace),'exec_lines':exec_lines,'successful_forbidden_opens':forbidden,'successful_cwd_or_escape_calls':changes,'withdrawal_seen':withdraw},status='observed_exact' if comp['exact'] and not forbidden and not changes and withdraw and len(exec_lines)==2 else 'observed_mismatch');write(o/'receipt.json',row);return row
for id in ('uv-26','uv-29','uv-extra-02'):
 a,raw,o,w,env,argv,before=fixture(id,'stock');r=capture(argv,w,env,30,o,'run.');comp=compare(raw,o,before,r,w);r.update(comparison=comp,original_normalized_invocation=a['normalized_invocation']);write(o/'receipt.json',r);summary['cases'][id]={'stock':r};print('STOCK',id,r['exit_code'],'exact',comp['exact'],flush=True)
for id in ('uv-26','uv-29'):
 for tier in ('O0','O3'):
  r=native(id,tier);summary['cases'][id][tier]=r;print('NATIVE',id,tier,r['status'],'build',r['build']['exit_code'],'run',r.get('run',{}).get('exit_code'),flush=True)
 d=direct_gate(id);g=imported_gate(id);summary['cases'][id]['direct_gate']=d;summary['cases'][id]['imported_gate']=g;print('GATES',id,'direct',d['rejected'],'imported',g['rejected'],flush=True)
a,raw,o,w,env,argv,before=fixture('uv-extra-02','module-build-probe')
for opt in ('0','3'):
 image=o/('module-O'+opt+'.exe');cmd=[str(cli),'build','-m','uv_build','--no-cache','--opt-level',opt,'--runtime',str(runtime),'--clang',str(clang),'-o',str(image)];r=capture(cmd,w,env,30,o,'O'+opt+'.');r.update(image_exists=image.exists(),diagnostic=(o/('O'+opt+'.stderr')).read_text(errors='replace'));summary['cases']['uv-extra-02']['O'+opt]=r;print('MODULE',opt,'exit',r['exit_code'],'image',r['image_exists'],'diagnostic',r['diagnostic'].strip()[:150],flush=True)
write(base/'summary.json',summary);print('COMPLETE',base/'summary.json',flush=True)
