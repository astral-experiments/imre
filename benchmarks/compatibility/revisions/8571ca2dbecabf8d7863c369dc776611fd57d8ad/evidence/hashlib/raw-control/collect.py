"""Family-local, bounded finite hashlib capture and source-withdrawal controls."""
from __future__ import annotations
import hashlib,json,os,pathlib,shutil,subprocess,sys,time
R=pathlib.Path('/workspace/imre-hashlib-strict'); PIN='8571ca2dbecabf8d7863c369dc776611fd57d8ad'; E=R/'benchmarks/compatibility/revisions'/PIN/'evidence/hashlib/raw-control'; S=pathlib.Path('/workspace/scratch/hashlib-strict'); CP='/workspace/scratch/python/cpython-3.14.5-linux-x86_64-gnu/bin/python3.14'; STOCK_LIB='/workspace/scratch/cpython-5607950ef232dad16d75c0cf53101d9649d89115/Lib'; CLI='/workspace/imre/target/release/imre'; RT='/workspace/imre/target/release/libimre_runtime.a'; CL='/workspace/.toolchains/imre-x86_64-unknown-linux-gnu/imre-toolchain/bin/clang'; L=E/'launcher.py'; PROBE=S/'raw-control-probe'; TREE=S/'fixtures/Lib/test'; LEAF=TREE/'test_hashlib.py'; STASH=pathlib.Path(str(TREE)+'.raw-control-stash'); SHA=lambda b:hashlib.sha256(b).hexdigest()
sys.path.insert(0,str(R/'benchmarks/compatibility')); from collect_unittest_results import parse
COMMON={'PYTHONDONTWRITEBYTECODE':'1','PYTHON_COLORS':'0','NO_COLOR':'1','PATH':str(pathlib.Path(CL).parent)+':'+os.environ['PATH'],'TMPDIR':str(S/'tmp')}
def env(tier): return {**COMMON,'PYTHONPATH':STOCK_LIB} if tier=='stock' else {**COMMON,'PYTHONPATH':str(TREE.parent),'IMRE_PYTHON':CLI,'IMREFLAGS':'--opt-level '+tier[1:]}
def write_json(p,obj): p.write_text(json.dumps(obj,indent=2)+'\n')
def invoke(argv,cwd,overrides,timeout,stem):
 start=time.monotonic(); timed=False
 try: p=subprocess.run(argv,cwd=cwd,env={**os.environ,**overrides},stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=timeout); code=p.returncode; out=p.stdout; err=p.stderr
 except subprocess.TimeoutExpired as exc: timed=True; code=None; out=exc.stdout or b''; err=exc.stderr or b''
 (E/(stem+'.stdout')).write_bytes(out); (E/(stem+'.stderr')).write_bytes(err)
 return {'argv':argv,'cwd':str(cwd),'environment':overrides,'timeout_seconds':timeout,'seconds':round(time.monotonic()-start,3),'exit_code':code,'timed_out':timed,'stdout_sha256':SHA(out),'stderr_sha256':SHA(err)}
def summarize(meta,stem):
 parsed=parse((E/(stem+'.stderr')).read_bytes(),meta['timed_out']); meta.update({k:parsed[k] for k in ('collection','inventory_complete','summary')}); return parsed

def replay():
 receipt=R/'benchmarks/compatibility/revisions'/PIN/'stdlib/hashlib.jsonl'; rows=[json.loads(l) for l in receipt.read_text().splitlines()]; source=rows[0]['source']; platform={'os':'linux','arch':'x86_64','release':os.uname().release}; version=subprocess.check_output([CP,'-VV'],text=True).strip(); run={'family':'hashlib','revision':PIN,'source':source,'platform':platform,'launcher_sha256':SHA(L.read_bytes()),'stock_version':version,'stock_python_sha256':SHA(pathlib.Path(CP).read_bytes()),'runs':{}}; a=invoke([CP,'-B',str(L)],S/'empty',env('stock'),180,'stock'); parsed=summarize(a,'stock'); run['runs']['stock']=a; capture={**a,'launcher_sha256':run['launcher_sha256'],'python_sha256':run['stock_python_sha256'],'version':version,'source_commit':source['commit'],'source_archive_sha256':SHA((E/'stock.source.py').read_bytes())};write_json(E/'stock.capture.json',capture)
 keys=[(x['id'],x['occurrence']) for x in parsed['originals']]; frozen=[(x['id'],x['occurrence']) for x in rows]; print('stock',a['exit_code'],a['collection'],a['seconds'],'ordered_frozen_match',keys==frozen,flush=True); assert a['exit_code']==0 and keys==frozen
 for tier in ('O0','O3'):
  opt=tier[1:]; image=S/(tier+'.exe'); gate=json.loads((E/'gate.partial.json').read_text()); command=gate['command'][:]; command[command.index('--opt-level')+1]=opt; command[command.index('-o')+1]=str(image); build=invoke(command,R,env(tier),300,tier+'.build'); print(tier,'build',build['exit_code'],build['seconds'],flush=True); assert build['exit_code']==0 and not build['timed_out'] and image.is_file(); meta=invoke([str(image)],S/'empty',env(tier),180,tier); parsed=summarize(meta,tier); meta['build']=build;meta['image_sha256']=SHA(image.read_bytes());run['runs'][tier]=meta; print(tier,'run',meta['exit_code'],meta['collection'],meta['seconds'], 'outcomes',dict(__import__('collections').Counter(r['outcome'] for r in parsed['originals'])),flush=True); assert meta['exit_code']==0 and [(r['id'],r['occurrence']) for r in parsed['originals']]==keys; elf=invoke(['readelf','-d',str(image)],R,{},30,tier+'.readelf');meta['withdrawal']={'readelf':elf}
 write_json(E/'run.json',run)

def withdraw():
 run=json.loads((E/'run.json').read_text());
 for tier in ('O0','O3'):
  image=run['runs'][tier]['argv'][0]; trace=['strace','-f','-qq','-s','512','-e','trace=execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,exit_group','-o',str(E/(tier+'.trace')),str(PROBE),'withdraw',str(TREE),str(STASH),str(LEAF),image]; meta=invoke(trace,S/'empty',env(tier),180,tier+'.withdraw'); run['runs'][tier]['withdrawal'].update({'supervisor_argv':trace,'supervisor_exit':meta['exit_code'],'seconds':meta['seconds']}); print(tier,'withdraw',meta['exit_code'],meta['seconds'],'identical', (E/(tier+'.withdraw.stderr')).read_bytes()==(E/(tier+'.stderr')).read_bytes(),flush=True); assert meta['exit_code']==0 and not STASH.exists() and LEAF.is_file() and (E/(tier+'.withdraw.stderr')).read_bytes()==(E/(tier+'.stderr')).read_bytes() and not (E/(tier+'.withdraw.stdout')).read_bytes()
 write_json(E/'run.json',run)
if __name__=='__main__': {'replay':replay,'withdraw':withdraw}[sys.argv[1]]()
