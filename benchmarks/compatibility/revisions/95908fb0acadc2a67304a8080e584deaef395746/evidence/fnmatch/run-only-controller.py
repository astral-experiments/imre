"""Retrospective run-only config correction; never invokes an Imre build."""
import hashlib, json, os, pathlib, signal, stat, subprocess, sys, time
C=pathlib.Path(__file__).resolve().parent; P=C.parent; R=C/'raw'; R.mkdir(exist_ok=False)
PRODUCT=pathlib.Path('/workspace/scratch/imre-95908-transferred-stdlib-v1/product'); CLI=str(PRODUCT/'imre')
PY='/workspace/scratch/python/cpython-3.14.5-linux-x86_64-gnu/bin/python3.14'
L=P/'launch/launcher.py'; A=P/'launch/stock.source.py'; LIB=P/'fixtures/Lib'; TREE=LIB/'test'; LEAF=TREE/'test_fnmatch.py'; STASH=pathlib.Path(str(TREE)+'.raw-control-stash'); PROBE=C/'raw-control-probe-child-env'; CWD=P/'empty'; TMP=P/'tmp'
SYS='execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,waitid,exit,exit_group,chdir,fchdir,close,close_range,dup,dup2,dup3,fcntl,clone,clone3,fork,vfork'
BASE={'PYTHONPATH':str(LIB),'PYTHONDONTWRITEBYTECODE':'1','PYTHON_COLORS':'0','NO_COLOR':'1','TMPDIR':str(TMP),'PATH':str(PRODUCT/'imre-toolchain/bin')}
EXPECTED={'O0':'8a3759dd8908ebc074f0f232ac0503c9494623438fc6994055b1c2f98bdd6bf8','O3':'2c707263143609d04408b2638c63bebcdb71af0185277a7d054edfbc07fdb460'}
def sha(p):
 h=hashlib.sha256()
 with open(p,'rb') as f:
  while b:=f.read(1024*1024): h.update(b)
 return h.hexdigest()
def dump(p,v): p.write_text(json.dumps(v,indent=2)+'\n')
def effects():
 rows=[]
 for root in (CWD,TMP):
  for x in sorted(root.rglob('*')):
   st=x.lstat(); row={'root':root.name,'path':str(x.relative_to(root)),'mode':stat.S_IMODE(st.st_mode),'kind':'file' if x.is_file() else 'dir' if x.is_dir() else 'other'}
   if x.is_file(): row['sha256']=sha(x)
   rows.append(row)
 return rows
CONTROLLER=sha(__file__)
def invoke(name,argv,env,childenv,trace=None):
 before=effects(); start=time.monotonic(); p=subprocess.Popen(argv,cwd=CWD,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True); timed=False
 try: out,err=p.communicate(timeout=60)
 except subprocess.TimeoutExpired:
  timed=True; os.killpg(p.pid,signal.SIGKILL); out,err=p.communicate()
 (R/(name+'.stdout')).write_bytes(out); (R/(name+'.stderr')).write_bytes(err); after=effects(); dump(R/(name+'.effects.before.json'),before); dump(R/(name+'.effects.after.json'),after)
 m={'controller_sha256':CONTROLLER,'argv':argv,'cwd':str(CWD),'environment':childenv,'timeout_seconds':60,'seconds':round(time.monotonic()-start,3),'exit_code':p.returncode,'timed_out':timed,'stdout_sha256':sha(R/(name+'.stdout')),'stderr_sha256':sha(R/(name+'.stderr')),'effects_before_sha256':sha(R/(name+'.effects.before.json')),'effects_after_sha256':sha(R/(name+'.effects.after.json')),'effects_equal':before==after}
 if trace: m.update(supervisor_environment=env,trace_sha256=sha(trace),trace_bytes=trace.stat().st_size)
 dump(R/(name+'.json'),m); print(name,p.returncode,m['seconds'],flush=True)
 return m
for tier in EXPECTED: assert sha(P/(tier+'.exe'))==EXPECTED[tier]
assert sha(L)=='b765beaf627cce312f7e180ecce0c328fccde487a9064bef0c7a27446302a560' and sha(A)==sha(LEAF)=='b6218bf75e1920ca273339589b88e522969184dc02eefe5272e1798fa7cc2f0c'
assert not STASH.exists() and all(not pathlib.Path(str(x)+'.raw-control-stash').exists() for x in (L,A))
stock=invoke('stock',[PY,'-B',str(L)],BASE,BASE)
assert stock['exit_code']==0 and not stock['timed_out'] and stock['effects_equal']
for suffix in ('stdout','stderr','effects.before.json','effects.after.json'): assert (R/('stock.'+suffix)).read_bytes()==(P/'raw'/('stock.'+suffix)).read_bytes(),suffix
runs={'stock':stock}
for tier in ('O0','O3'):
 image=P/(tier+'.exe'); child={**BASE,'IMRE_PYTHON':CLI,'IMREFLAGS':'--opt-level '+tier[1:]}; parent={**child,'IMRE_WITHDRAW_LAUNCHER':str(L),'IMRE_WITHDRAW_ARCHIVE':str(A)}; trace=R/(tier+'.trace'); before={str(x):sha(x) for x in (image,LEAF,L,A)}
 argv=['/usr/bin/strace','-f','-y','-v','-qq','-s','512','-e','trace='+SYS,'-o',str(trace),str(PROBE),'withdraw',str(TREE),str(STASH),str(LEAF),str(image)]
 try: m=invoke(tier+'.withdraw',argv,parent,child,trace)
 finally:
  for x in (A,L,TREE):
   stash=pathlib.Path(str(x)+'.raw-control-stash')
   if stash.exists() and not x.exists(): stash.rename(x)
 after={str(x):sha(x) if x.is_file() else None for x in (image,LEAF,L,A)}; m.update(before=before,after=after,restored=before==after,image_sha256=sha(image),supervisor_sha256=sha(PROBE)); dump(R/(tier+'.withdraw.json'),m)
 assert m['exit_code']==0 and not m['timed_out'] and m['effects_equal'] and m['restored'] and sha(image)==EXPECTED[tier]
 for suffix in ('stdout','stderr','effects.before.json','effects.after.json'): assert (R/(tier+'.withdraw.'+suffix)).read_bytes()==(P/'raw2'/(tier+'.withdraw.'+suffix)).read_bytes(),(tier,suffix)
 meta=dict(m); meta['capture_argv']=meta.pop('argv'); meta['argv']=[str(image)]; runs[tier]=meta
run={'revision':'95908fb0acadc2a67304a8080e584deaef395746','family':'fnmatch','runs':runs,'gate_evidence':{'controller_sha256':sha(P/'capture2.py'),'probe_sha256':sha(P/'raw-control-probe'),'direct':str(P/'raw2/direct.gate.json'),'imported':str(P/'raw2/imported.gate.json')},'withdrawal_evidence':{'controller_sha256':CONTROLLER,'probe_sha256':sha(PROBE),'probe_source_sha256':sha(C/'raw_control_probe_child_env.c')}}; dump(C/'run.json',run)
sys.path.insert(0,'/workspace/scratch/imre-original84-authority-draft/benchmarks/compatibility'); import raw_control,scorecard
raw_control.comparable_run(run,str(LIB),CLI,scorecard.require)
print('generic comparable_run: PASS',flush=True)
