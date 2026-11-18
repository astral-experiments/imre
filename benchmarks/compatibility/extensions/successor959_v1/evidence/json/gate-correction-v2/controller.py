"""JSON decode same-positive-graph gate supplement; never builds or runs a positive image."""
import hashlib,json,os,pathlib,signal,stat,subprocess,sys,time
G=pathlib.Path(__file__).resolve().parent;R=G.parent;RAW=G/'raw';L=R/'launch/launcher.py';POISON=L.parent/'json_decode_gate_poison.py';PROBE=pathlib.Path('/workspace/scratch/fnmatch-95908-replay-v1/correction-v2/raw-control-probe-child-env');SYS='execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,waitid,exit,exit_group,chdir,fchdir,close,close_range,dup,dup2,dup3,fcntl,clone,clone3,fork,vfork';EXPECTED='55367f06f64b7441708387dba7625753f4312e0a70fdf5b37208e40dd35fee0b'
sys.path.insert(0,'/workspace/scratch/imre-959-activation/benchmarks/compatibility');import scorecard,raw_control
def sha_bytes(b):return hashlib.sha256(b).hexdigest()
def sha(p):
 h=hashlib.sha256()
 with open(p,'rb') as f:
  while b:=f.read(1024*1024):h.update(b)
 return h.hexdigest()
def dump(p,v):p.write_text(json.dumps(v,indent=2,sort_keys=True)+'\n')
def effects():
 out=[]
 for root in (R/'empty',R/'tmp'):
  for p in sorted(root.rglob('*')):
   st=p.lstat();x={'root':root.name,'path':str(p.relative_to(root)),'kind':'file' if p.is_file() else 'directory' if p.is_dir() else 'other','mode':stat.S_IMODE(st.st_mode)}
   if p.is_file():x['sha256']=sha(p)
   out.append(x)
 return out
original=L.read_bytes();assert sha_bytes(original)==EXPECTED and original.count(b'import unittest\n')==1 and not POISON.exists();controller_sha=sha(__file__);probe_sha=sha(PROBE);source_sha=sha(G/'probe-source.c');probe_build=json.loads((G/'probe-build.json').read_text());assert source_sha==probe_build['source_sha256'] and probe_sha==probe_build['executable_sha256'];assessment_sha=sha(R/'assessment.json');images_before={t:sha(R/'images'/f'{t}.exe') for t in ('O0','O3')};records=[]
try:
 for tier in ('O0','O3'):
  positive=json.loads((R/'raw'/f'{tier}.build.json').read_text());assert positive['image_sha256']==images_before[tier] and positive['launcher_sha256']==EXPECTED
  for kind in ('direct','imported'):
   assert L.read_bytes()==original and not POISON.exists();addition=b'status: int = True\n' if kind=='direct' else b'import json_decode_gate_poison\n';negative=original.replace(b'import unittest\n',b'import unittest\n'+addition,1);assert negative.replace(b'import unittest\n'+addition,b'import unittest\n',1)==original;(G/'negative'/f'{kind}.launcher.py').write_bytes(negative)
   if kind=='imported':(G/'negative/poison.py').write_bytes(b'status: int = True\n');POISON.write_bytes(b'status: int = True\n')
   L.write_bytes(negative);stem=f'{tier}.{kind}';out=G/f'{stem}.exe';trace=RAW/f'{stem}.trace';cmd=list(positive['argv']);cmd[cmd.index('-o')+1]=str(out);assert not out.exists() and cmd[2]==str(L) and scorecard.comparable_build(cmd)==scorecard.comparable_build(positive['argv']);scorecard.matched_build(cmd,tier,positive['argv'][0],scorecard.runtime_argument(positive['argv']));env=positive['environment'];before=effects();start=time.monotonic();argv=['/usr/bin/strace','-f','-y','-v','-qq','-s','512','-e','trace='+SYS,'-o',str(trace),str(PROBE),'gate',str(out),*cmd];p=subprocess.Popen(argv,cwd=positive['cwd'],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True);timed=False
   try:stdout,stderr=p.communicate(timeout=90)
   except subprocess.TimeoutExpired:timed=True;os.killpg(p.pid,signal.SIGKILL);stdout,stderr=p.communicate()
   elapsed=round(time.monotonic()-start,3);(RAW/f'{stem}.stdout').write_bytes(stdout);(RAW/f'{stem}.stderr').write_bytes(stderr);after=effects();dump(RAW/f'{stem}.effects.before.json',before);dump(RAW/f'{stem}.effects.after.json',after);txt=trace.read_text();diag=stderr.decode();first,child=raw_control.execs(txt.splitlines(),scorecard.require);actual=raw_control.exec_environment(txt.splitlines()[child[0]],scorecard.require)
   if kind=='direct':raw_control.trace_direct_gate(txt,cmd,str(PROBE),diag,scorecard.require,expected_exit=1)
   else:raw_control.trace_gate(txt,cmd,str(PROBE),str(POISON),diag,scorecard.require,expected_exit=1)
   assert p.returncode==0 and not timed and not out.exists() and actual==env and before==after and sha(L)==sha_bytes(negative)
   m={'tier':tier,'kind':kind,'controller_sha256':controller_sha,'probe_path':str(PROBE),'probe_sha256':probe_sha,'probe_source_sha256':source_sha,'original_launcher_path':str(L),'original_launcher_sha256':EXPECTED,'negative_launcher_sha256':sha_bytes(negative),'source_delta':addition.decode(),'poison_path':str(POISON) if kind=='imported' else None,'poison_sha256':sha(POISON) if kind=='imported' else None,'positive_build_metadata_sha256':sha(R/'raw'/f'{tier}.build.json'),'positive_build_argv':positive['argv'],'build_argv':cmd,'comparable_build':True,'matched_build':True,'capture_argv':argv,'cwd':positive['cwd'],'environment':env,'actual_compiler_child_environment':actual,'timeout_seconds':90,'seconds':elapsed,'supervisor_exit_code':p.returncode,'compiler_child_exit_code':1,'timed_out':timed,'image_absent_before':True,'image_absent_after':not out.exists(),'stdout_sha256':sha_bytes(stdout),'stderr_sha256':sha_bytes(stderr),'trace_sha256':sha(trace),'trace_bytes':trace.stat().st_size,'effects_before_sha256':sha(RAW/f'{stem}.effects.before.json'),'effects_after_sha256':sha(RAW/f'{stem}.effects.after.json'),'effects_equal':before==after}
   L.write_bytes(original)
   if kind=='imported':POISON.unlink()
   m.update(launcher_restored_sha256=sha(L),poison_absent_after=not POISON.exists());assert m['launcher_restored_sha256']==EXPECTED and m['poison_absent_after'];dump(RAW/f'{stem}.json',m);records.append(m);print(stem,'compiler exit 1, no image, same graph',elapsed,flush=True)
finally:
 if L.read_bytes()!=original:L.write_bytes(original)
 if POISON.exists():POISON.unlink()
assert sha(L)==EXPECTED and not POISON.exists() and sha(R/'assessment.json')==assessment_sha;images_after={t:sha(R/'images'/f'{t}.exe') for t in images_before};assert images_after==images_before
summary={'schema':'imre.compatibility.json-decode-gate-correction.v2','revision':'95908fb0acadc2a67304a8080e584deaef395746','controller_sha256':controller_sha,'original_assessment_sha256':assessment_sha,'positive_images_before':images_before,'positive_images_after':images_after,'launcher_restored_sha256':sha(L),'poison_absent':not POISON.exists(),'controls':[{k:m[k] for k in ('tier','kind','negative_launcher_sha256','poison_sha256','comparable_build','matched_build','compiler_child_exit_code','image_absent_before','image_absent_after','trace_sha256','seconds')} for m in records],'credit':0,'effects_scope':['empty','tmp'],'limits':'A gate-only supplementary capture, not a contemporaneous part of the original assessment; effect snapshots cover only the two named private roots.'};dump(G/'summary.json',summary);assets={str(p.relative_to(G)):{'bytes':p.stat().st_size,'sha256':sha(p)} for p in sorted(G.rglob('*')) if p.is_file() and p.name not in {'export-manifest.json','COMPLETE.json'}};dump(G/'export-manifest.json',assets);seal={'schema':'imre.compatibility.sealed-supplement.v1','role':'json-decode-959-same-positive-graph-gates','credit':0,'manifest_sha256':sha(G/'export-manifest.json'),'assets':len(assets),'source_launcher_sha256':EXPECTED,'original_assessment_sha256':assessment_sha};dump(G/'COMPLETE.json',seal);print('COMPLETE',sha(G/'COMPLETE.json'),len(assets),flush=True)
