"""Bounded logging capture and raw direct/imported poison and withdrawal controls."""
from __future__ import annotations
import hashlib,json,os,pathlib,subprocess,sys,time
R=pathlib.Path(__file__).resolve().parents[7]; PIN='8571ca2dbecabf8d7863c369dc776611fd57d8ad'; E=pathlib.Path(__file__).resolve().parent; S=pathlib.Path('/workspace/scratch/logging-strict-8571'); CP='/workspace/scratch/python/cpython-3.14.5-linux-x86_64-gnu/bin/python3.14'; STOCK_LIB='/workspace/scratch/cpython-5607950ef232dad16d75c0cf53101d9649d89115/Lib'; CLI='/workspace/scratch/imre-target-8571/release/imre'; RT='/workspace/scratch/imre-target-8571/release/libimre_runtime.a'; CL='/workspace/.toolchains/imre-x86_64-unknown-linux-gnu/imre-toolchain/bin/clang'; L=E/'launcher.py'; PROBE=S/'raw-control-probe'; TREE=pathlib.Path('/workspace/scratch/glob-logging-8571/fixtures/Lib/test'); LEAF=TREE/'test_logging.py'; STASH=pathlib.Path(str(TREE)+'.raw-control-stash'); SHA=lambda b:hashlib.sha256(b).hexdigest()
sys.path.insert(0,str(R/'benchmarks/compatibility'));from collect_unittest_results import parse
COMMON={'PYTHONDONTWRITEBYTECODE':'1','PYTHON_COLORS':'0','NO_COLOR':'1','PATH':str(pathlib.Path(CL).parent)+':'+os.environ['PATH'],'TMPDIR':str(S/'tmp')}
def env(t):return {**COMMON,'PYTHONPATH':STOCK_LIB} if t=='stock' else {**COMMON,'PYTHONPATH':str(TREE.parent),'IMRE_PYTHON':CLI,'IMREFLAGS':'--opt-level '+t[1:]}
def j(p,o):p.write_text(json.dumps(o,indent=2)+'\n')
def cmd(image,tier='O0'):return [CLI,'build',str(L),'--module-root',str(E),'--extension-path',str(TREE.parent),'--no-cache','--quiet','--opt-level',tier[1:],'--runtime',RT,'--clang',CL,'--include-module','test.test_logging','-o',str(image)]
def trace(stem,*args):return ['/usr/bin/strace','-f','-qq','-s','512','-e','trace=execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,exit_group','-o',str(E/(stem+'.trace')),str(PROBE),*map(str,args)]
def invoke(argv,cwd,overrides,timeout,stem):
 start=time.monotonic();p=subprocess.run(argv,cwd=cwd,env={**os.environ,**overrides},capture_output=True,timeout=timeout);(E/(stem+'.stdout')).write_bytes(p.stdout);(E/(stem+'.stderr')).write_bytes(p.stderr)
 return {'argv':argv,'cwd':str(cwd),'environment':overrides,'timeout_seconds':timeout,'seconds':round(time.monotonic()-start,3),'exit_code':p.returncode,'timed_out':False,'stdout_sha256':SHA(p.stdout),'stderr_sha256':SHA(p.stderr)}
def gates():
 positive=L.read_bytes()
 for mode,negative in [('direct','direct.launcher.py'),('gate','gate.launcher.py')]:
  image=S/(mode+'.exe');assert not image.exists();command=cmd(image)
  try:
   L.write_bytes((E/negative).read_bytes());(E/(mode+'-files.raw')).write_text(subprocess.check_output(['sha256sum',str(L),str(E/'logging_poison.py')],text=True));m=invoke(trace(mode,'gate',image,*command),R,env('O0'),180,mode)
  finally:L.write_bytes(positive)
  j(E/(mode+'.command.json'),{'command':command,'environment':env('O0'),'supervisor_exit':m['exit_code'],'image_exists':image.exists()});print(mode,m['exit_code'],'image',image.exists(),(E/(mode+'.stderr')).read_text()[:350].replace('\n',' | '),flush=True)
  assert m['exit_code']==0 and not image.exists() and b'expected int, got bool' in (E/(mode+'.stderr')).read_bytes()
 (E/'positive.raw').write_text(subprocess.check_output(['sha256sum',str(L)],text=True))
def replay():
 rows=[json.loads(x) for x in (E.parents[2]/'stdlib/logging.jsonl').read_text().splitlines()];source=rows[0]['source'];version=subprocess.check_output([CP,'-VV'],text=True).strip();run={'family':'logging','revision':PIN,'source':source,'platform':{'os':'linux','arch':'x86_64','release':os.uname().release},'launcher_sha256':SHA(L.read_bytes()),'stock_version':version,'stock_python_sha256':SHA(pathlib.Path(CP).read_bytes()),'runs':{}}
 full=invoke([CP,'-B',str(E/'stock-all-launcher.py')],S/'empty',env('stock'),180,'stock-all');ap=parse((E/'stock-all.stderr').read_bytes(),False);j(E/'stock-all.json',full);print('stock all',full['exit_code'],len(ap['originals']),{v:sum(x['outcome']==v for x in ap['originals']) for v in ('pass','skip','error')},flush=True);assert full['exit_code']==0 and [(x['id'],x['occurrence']) for x in ap['originals']]==[(x['id'],x['occurrence']) for x in rows]
 m=invoke([CP,'-B',str(L)],S/'empty',env('stock'),180,'stock');p=parse((E/'stock.stderr').read_bytes(),False);m.update({k:p[k] for k in ('collection','inventory_complete','summary')});run['runs']['stock']=m;j(E/'stock.capture.json',{**m,'launcher_sha256':run['launcher_sha256'],'python_sha256':run['stock_python_sha256'],'version':version,'source_commit':source['commit'],'source_archive_sha256':SHA((E/'stock.source.zip').read_bytes())});keys=[(x['id'],x['occurrence']) for x in p['originals']];print('stock finite',m['exit_code'],len(keys),flush=True);assert m['exit_code']==0 and len(keys)==1
 for t in ('O0','O3'):
  image=S/(t+'.exe');b=invoke(cmd(image,t),R,env(t),600,t+'.build');print(t,'build',b['exit_code'],b['seconds'],flush=True);assert b['exit_code']==0 and image.is_file();m=invoke([str(image)],S/'empty',env(t),180,t);p=parse((E/(t+'.stderr')).read_bytes(),False);m.update({k:p[k] for k in ('collection','inventory_complete','summary')});m['build']=b;m['image_sha256']=SHA(image.read_bytes());m['withdrawal']={'readelf':invoke(['readelf','-d',str(image)],R,{},30,t+'.readelf')};run['runs'][t]=m;print(t,'run',m['exit_code'],len(p['originals']),{v:sum(x['outcome']==v for x in p['originals']) for v in ('pass','skip','error','fail')},flush=True);assert m['exit_code']==0 and [(x['id'],x['occurrence']) for x in p['originals']]==keys
 j(E/'run.json',run)
def withdraw():
 run=json.loads((E/'run.json').read_text())
 for t in ('O0','O3'):
  image=run['runs'][t]['argv'][0];argv=trace(t,'withdraw',TREE,STASH,LEAF,image);hidden={**env(t),'PATH':str(pathlib.Path(CL).parent),'IMRE_WITHDRAW_LAUNCHER':str(L),'IMRE_WITHDRAW_ARCHIVE':str(E/'stock.source.zip')};m=invoke(argv,S/'empty',hidden,180,t+'.withdraw');run['runs'][t]['withdrawal'].update({'supervisor_argv':argv,'supervisor_exit':m['exit_code'],'seconds':m['seconds'],'environment':hidden});print(t,'withdraw',m['exit_code'],'identical',(E/(t+'.withdraw.stderr')).read_bytes()==(E/(t+'.stderr')).read_bytes(),flush=True);assert m['exit_code']==0 and LEAF.is_file() and not STASH.exists() and (E/(t+'.withdraw.stderr')).read_bytes()==(E/(t+'.stderr')).read_bytes() and not (E/(t+'.withdraw.stdout')).read_bytes()
 j(E/'run.json',run)
if __name__=='__main__':{'gates':gates,'replay':replay,'withdraw':withdraw}[sys.argv[1]]()
