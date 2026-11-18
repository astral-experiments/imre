import hashlib,json,os,pathlib,signal,stat,subprocess,sys,time
R=pathlib.Path(__file__).resolve().parent; RAW=R/'raw'; P=pathlib.Path('/workspace/scratch/imre-95908-transferred-stdlib-v1/product'); CLI=str(P/'imre'); PY='/workspace/scratch/python/cpython-3.14.5-linux-x86_64-gnu/bin/python3.14'; L=R/'launch/launcher.py'; A=R/'launch/stock.source.py'; LIB=R/'fixtures/Lib'; TREE=LIB/'test'; LEAF=TREE/'test_json/test_decode.py'; CWD=R/'empty'; TMP=R/'tmp'; PROBE='/workspace/scratch/fnmatch-95908-replay-v1/correction-v2/raw-control-probe-child-env'; SYS='execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,waitid,exit,exit_group,chdir,fchdir,close,close_range,dup,dup2,dup3,fcntl,clone,clone3,fork,vfork'; BASE={'PYTHONPATH':str(LIB),'PYTHONDONTWRITEBYTECODE':'1','PYTHON_COLORS':'0','NO_COLOR':'1','TMPDIR':str(TMP),'PATH':str(P/'imre-toolchain/bin')}
def sha(p):
 h=hashlib.sha256()
 with open(p,'rb') as f:
  while b:=f.read(1024*1024):h.update(b)
 return h.hexdigest()
def dump(p,d):p.write_text(json.dumps(d,indent=2,sort_keys=True)+'\n')
def effects():
 out=[]
 for root in (CWD,TMP):
  for p in sorted(root.rglob('*')):
   st=p.lstat();d={'root':root.name,'path':str(p.relative_to(root)),'mode':stat.S_IMODE(st.st_mode),'kind':'file' if p.is_file() else 'directory' if p.is_dir() else 'other'}
   if p.is_file():d['sha256']=sha(p)
   out.append(d)
 return out
def capture(name,argv,env,timeout,child=None,trace=None):
 before=effects();start=time.monotonic();p=subprocess.Popen(argv,cwd=CWD,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True);timed=False
 try:out,err=p.communicate(timeout=timeout)
 except subprocess.TimeoutExpired:timed=True;os.killpg(p.pid,signal.SIGKILL);out,err=p.communicate()
 (RAW/(name+'.stdout')).write_bytes(out);(RAW/(name+'.stderr')).write_bytes(err);after=effects();dump(RAW/(name+'.effects.before.json'),before);dump(RAW/(name+'.effects.after.json'),after);m={'controller_sha256':sha(__file__),'capture_argv':argv,'cwd':str(CWD),'environment':env if child is None else child,'timeout_seconds':timeout,'seconds':round(time.monotonic()-start,3),'exit_code':p.returncode,'timed_out':timed,'stdout_sha256':sha(RAW/(name+'.stdout')),'stderr_sha256':sha(RAW/(name+'.stderr')),'effects_before_sha256':sha(RAW/(name+'.effects.before.json')),'effects_after_sha256':sha(RAW/(name+'.effects.after.json')),'effects_equal':before==after}
 if trace:m.update(trace_sha256=sha(trace),trace_bytes=trace.stat().st_size)
 dump(RAW/(name+'.json'),m);print(name,p.returncode,m['seconds'],'timeout',timed,flush=True);return m
def traced(trace,argv):return ['/usr/bin/strace','-f','-y','-v','-qq','-s','512','-e','trace='+SYS,'-o',str(trace),*argv]
def buildargv(source,tier,image):return [CLI,'build',str(source),'--module-root',str(source.parent),'--extension-path',str(LIB),'--no-cache','--quiet','--opt-level',tier[1:],'--runtime',str(P/'libimre_runtime.a'),'--clang',str(P/'imre-toolchain/bin/clang'),'-o',str(image)]
mode=sys.argv[1]
if mode=='stock':
 tr=RAW/'stock.trace';argv=[PY,'-B',str(L)];m=capture('stock',traced(tr,argv),BASE,90,BASE,tr);m['argv']=argv;dump(RAW/'stock.json',m)
elif mode=='build':
 tier=sys.argv[2];image=R/'images'/f'{tier}.exe';argv=buildargv(L,tier,image);m=capture(tier+'.build',argv,{**BASE,'PATH':BASE['PATH']+':/usr/bin'},300);m['argv']=argv;m['launcher_sha256']=sha(L);m['source_sha256']=sha(LEAF);m['image_sha256']=sha(image) if image.exists() else None;dump(RAW/(tier+'.build.json'),m)
elif mode=='native':
 tier=sys.argv[2];image=R/'images'/f'{tier}.exe';child={**BASE,'IMRE_PYTHON':CLI,'IMREFLAGS':'--opt-level '+tier[1:]};parent={**child,'IMRE_WITHDRAW_LAUNCHER':str(L),'IMRE_WITHDRAW_ARCHIVE':str(A)};tr=RAW/(tier+'.trace');stash=pathlib.Path(str(TREE)+'.raw-control-stash');before={str(p):sha(p) for p in (image,LEAF,L,A)}
 try:m=capture(tier,traced(tr,[PROBE,'withdraw',str(TREE),str(stash),str(LEAF),str(image)]),parent,90,child,tr)
 finally:
  for p in (A,L,TREE):
   s=pathlib.Path(str(p)+'.raw-control-stash')
   if s.exists() and not p.exists():s.rename(p)
 after={str(p):sha(p) if p.is_file() else None for p in (image,LEAF,L,A)};m.update(argv=[str(image)],before=before,after=after,restored=before==after,image_sha256=sha(image),supervisor_sha256=sha(PROBE));dump(RAW/(tier+'.json'),m)
elif mode=='gate':
 tier,kind=sys.argv[2:];source=R/'controls'/('direct.py' if kind=='direct' else 'imported.py');image=R/'controls'/f'{tier}-{kind}.exe';argv=buildargv(source,tier,image);tr=RAW/f'{tier}.{kind}.trace';m=capture(f'{tier}.{kind}',traced(tr,[PROBE,'gate',str(image),*argv]),BASE,90,BASE,tr);m.update(build_argv=argv,image_exists=image.exists(),source_sha256=sha(source),supervisor_sha256=sha(PROBE));dump(RAW/f'{tier}.{kind}.json',m)
