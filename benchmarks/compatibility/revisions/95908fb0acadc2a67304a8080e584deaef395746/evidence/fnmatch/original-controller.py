import hashlib,json,os,pathlib,signal,stat,subprocess,time
P=pathlib.Path(__file__).resolve().parent;R=P/'raw2';R.mkdir();PROD=pathlib.Path('/workspace/scratch/imre-95908-transferred-stdlib-v1/product');CLI=str(PROD/'imre');RT=str(PROD/'libimre_runtime.a');CLANG=str(PROD/'imre-toolchain/bin/clang');PY='/workspace/scratch/python/cpython-3.14.5-linux-x86_64-gnu/bin/python3.14';L=P/'launch/launcher.py';A=P/'launch/stock.source.py';LIB=P/'fixtures/Lib';TREE=LIB/'test';LEAF=TREE/'test_fnmatch.py';STASH=pathlib.Path(str(TREE)+'.raw-control-stash');PROBE=str(P/'raw-control-probe');CWD=P/'empty';TMP=P/'tmp';SYSCALLS='execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,waitid,exit,exit_group,chdir,fchdir,close,close_range,dup,dup2,dup3,fcntl,clone,clone3,fork,vfork';BASE={'PYTHONPATH':str(LIB),'PYTHONDONTWRITEBYTECODE':'1','PYTHON_COLORS':'0','NO_COLOR':'1','TMPDIR':str(TMP),'PATH':str(pathlib.Path(CLANG).parent),'IMREFLAGS':''};sha=lambda b:hashlib.sha256(b).hexdigest();fsha=lambda p:sha(pathlib.Path(p).read_bytes());CONTROLLER=fsha(__file__)
def dump(path,obj):path.write_text(json.dumps(obj,indent=2)+'\n')
def effects():
 rows=[]
 for root in (CWD,TMP):
  for x in sorted(root.rglob('*')):
   s=x.lstat();row={'root':root.name,'path':str(x.relative_to(root)),'mode':stat.S_IMODE(s.st_mode),'kind':'file' if x.is_file() else 'dir' if x.is_dir() else 'other'}
   if x.is_file():row['sha256']=fsha(x)
   rows.append(row)
 return rows
def invoke(name,argv,env,timeout,trace=None):
 before=effects();start=time.monotonic();pr=subprocess.Popen(argv,cwd=CWD,env={**os.environ,**env},stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True);timed=False
 try:out,err=pr.communicate(timeout=timeout)
 except subprocess.TimeoutExpired:timed=True;os.killpg(pr.pid,signal.SIGKILL);out,err=pr.communicate()
 (R/(name+'.stdout')).write_bytes(out);(R/(name+'.stderr')).write_bytes(err);after=effects();dump(R/(name+'.effects.before.json'),before);dump(R/(name+'.effects.after.json'),after);m={'controller_sha256':CONTROLLER,'argv':argv,'cwd':str(CWD),'environment':env,'timeout_seconds':timeout,'seconds':round(time.monotonic()-start,3),'exit_code':pr.returncode,'timed_out':timed,'stdout_sha256':sha(out),'stderr_sha256':sha(err),'effects_before_sha256':fsha(R/(name+'.effects.before.json')),'effects_after_sha256':fsha(R/(name+'.effects.after.json')),'effects_equal':before==after}
 if trace:m.update(trace_sha256=fsha(trace),trace_bytes=trace.stat().st_size)
 dump(R/(name+'.json'),m);print(name,'exit',pr.returncode,'seconds',m['seconds'],'effects_equal',m['effects_equal'],flush=True);return m
def trace_cmd(path,*args):return ['/usr/bin/strace','-f','-y','-qq','-s','512','-e','trace='+SYSCALLS,'-o',str(path),*map(str,args)]
def build(image,tier):return [CLI,'build',str(L),'--module-root',str(L.parent),'--extension-path',str(LIB),'--no-cache','--quiet','--opt-level',tier[1:],'--runtime',RT,'--clang',CLANG,'--include-module','test.test_fnmatch','-o',str(image)]
BUILD={**BASE,'PATH':str(pathlib.Path(CLANG).parent)+':/usr/bin:/bin'}
positive=L.read_bytes();source_hash=fsha(LEAF);assert fsha(A)==source_hash
for mode in ('direct','imported'):
 image=P/(mode+'.gate.exe');assert not image.exists();poison=L.parent/'fnmatch_control_poison.py';assert not poison.exists();negative=positive.replace(b'import unittest\n',b'import unittest\n\nstatus: int = True\n',1) if mode=='direct' else positive.splitlines(keepends=True)[0]+b'import fnmatch_control_poison\n'+b''.join(positive.splitlines(keepends=True)[1:]);(R/(mode+'.negative.launcher.py')).write_bytes(negative);L.write_bytes(negative)
 try:
  if mode=='imported':poison.write_bytes(b'status: int = True\n');(R/'imported.poison.py').write_bytes(poison.read_bytes())
  tr=R/(mode+'.gate.trace');m=invoke(mode+'.gate',trace_cmd(tr,PROBE,'gate',image,*build(image,'O0')),BUILD,60,tr);m.update(negative_launcher_sha256=fsha(L),poison_sha256=fsha(poison) if poison.exists() else None,image_exists=image.exists(),compiler_sha256=fsha(CLI),runtime_sha256=fsha(RT));dump(R/(mode+'.gate.json'),m);assert m['exit_code']==0 and not image.exists() and b'expected int, got bool' in (R/(mode+'.gate.stderr')).read_bytes()
 finally:L.write_bytes(positive);poison.unlink(missing_ok=True)
for tier in ('O0','O3'):
 image=P/(tier+'.exe');assert not image.exists();m=invoke(tier+'.build',build(image,tier),BUILD,120);m.update(image_exists=image.exists(),image_sha256=fsha(image) if image.exists() else None,compiler_sha256=fsha(CLI),runtime_sha256=fsha(RT),source_sha256=fsha(LEAF),launcher_sha256=fsha(L));dump(R/(tier+'.build.json'),m);assert m['exit_code']==0 and image.exists();before={str(x):fsha(x) for x in (image,LEAF,L,A)};assert not STASH.exists();env={**BASE,'IMRE_PYTHON':CLI,'IMREFLAGS':'--opt-level '+tier[1:],'IMRE_WITHDRAW_LAUNCHER':str(L),'IMRE_WITHDRAW_ARCHIVE':str(A)};tr=R/(tier+'.trace')
 try:m=invoke(tier+'.withdraw',trace_cmd(tr,PROBE,'withdraw',TREE,STASH,LEAF,image),env,60,tr)
 finally:
  for x in (A,L,TREE):
   saved=pathlib.Path(str(x)+'.raw-control-stash')
   if saved.exists() and not x.exists():saved.rename(x)
 after={str(x):fsha(x) if x.is_file() else None for x in (image,LEAF,L,A)};m.update(before=before,after=after,restored=before==after);dump(R/(tier+'.withdraw.json'),m);assert m['exit_code']==0 and not m['timed_out'] and before==after
print('capture complete',flush=True)
