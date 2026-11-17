"""Focused, pinned JSON finite-cohort capture in Imre Cloud. No source edits."""
from __future__ import annotations
import hashlib,json,os,pathlib,subprocess,sys,time
ROOT=pathlib.Path('/workspace/imre-json'); PIN='8571ca2dbecabf8d7863c369dc776611fd57d8ad'; SNAP=ROOT/'benchmarks/compatibility/revisions'/PIN; OUT=SNAP/'evidence/json/strict'; SCR=pathlib.Path('/workspace/scratch/json-strict-8571'); EXT='/workspace/scratch/imre-cpython-test-fixtures/Lib'; STOCKLIB='/workspace/scratch/cpython-5607950ef232dad16d75c0cf53101d9649d89115/Lib'; PY='/workspace/scratch/cpython-build/python'; CLI='/workspace/imre/target/release/imre'; RT='/workspace/imre/target/release/libimre_runtime.a'; CLANG='/workspace/.toolchains/imre-x86_64-unknown-linux-gnu/imre-toolchain/bin/clang'; PROBE=str(SCR/'probe'); CWD=SCR/'empty'; CWD.mkdir(exist_ok=True); LIMIT=90
sys.path.insert(0,str(ROOT/'benchmarks/compatibility'));from generate_unittest_runner import generate;from collect_unittest_results import parse

def sha(p): return hashlib.file_digest(open(p,'rb'),'sha256').hexdigest()
def dump(p,o): p.write_text(json.dumps(o,indent=2)+'\n')
def cmd(argv,env=None,cwd=None,timeout=LIMIT):
 t=time.monotonic();p=subprocess.run(argv,env=env,cwd=cwd,capture_output=True,timeout=timeout);return p,round(time.monotonic()-t,3)
def capture(argv,env,d,tier):
 p,sec=cmd(argv,env,str(CWD));(d/f'{tier}.stderr').write_bytes(p.stderr);(d/f'{tier}.stdout').write_bytes(p.stdout);x=parse(p.stderr,False);meta={'argv':argv,'cwd':str(CWD),'environment':env,'timeout_seconds':LIMIT,'exit_code':p.returncode,'timed_out':False,'stderr_sha256':sha(d/f'{tier}.stderr'),'stdout_sha256':sha(d/f'{tier}.stdout'),'collection':x['collection'],'inventory_complete':x['inventory_complete'],'summary':x['summary'],'seconds':sec};return x,meta
def trace(d,label,args,env,cwd=None):
 argv=['strace','-f','-qq','-s','512','-e','trace=execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,exit_group','-o',str(d/f'{label}.trace'),PROBE,*args];p,sec=cmd(argv,env,cwd);(d/f'{label}.stderr').write_bytes(p.stderr);(d/f'{label}.stdout').write_bytes(p.stdout);return p,sec

def cohort(name,rows):
 d=OUT/name;d.mkdir(exist_ok=True);source=rows[0]['source'];assert all(r['source']==source and r['occurrence']==1 for r in rows);module='.'.join(pathlib.Path(source['path']).with_suffix('').parts[1:]);module=module.removesuffix('.__init__');ids=[r['id']for r in rows];base=generate([module]);selection='''def select_originals(source, target, wanted):
    for original in source:
        if isinstance(original, unittest.TestSuite):
            select_originals(original, target, wanted)
        elif original.id() in wanted:
            target.addTest(original)


''';base=base.replace('def collect():',f'WANTED = {tuple(ids)!r}\n\n\n'+selection+'def collect():').replace('    return suite\n','    selected = unittest.TestSuite()\n    select_originals(suite, selected, WANTED)\n    return selected\n',1);launcher=d/'positive_launcher.py';launcher.write_text(base);poison=d/'bad_status.py';poison.write_text('status: int = True\n');lines=launcher.read_bytes().splitlines(keepends=True);negative=lines[0]+b'import bad_status\n'+b''.join(lines[1:]);(d/'negative_launcher.py').write_bytes(negative);positive=launcher.read_bytes();output=SCR/'images'/f'{name}-gate.exe'
 build0=[CLI,'build',str(launcher),'--module-root',str(d),'--extension-path',EXT,'--no-cache','--quiet','--opt-level','0','--runtime',RT,'--clang',CLANG,'-o',str(output)];shellenv={k:v for k,v in os.environ.items() if k=='PATH'};common={'PATH':shellenv['PATH'],'PYTHONDONTWRITEBYTECODE':'1','PYTHON_COLORS':'0','NO_COLOR':'1'};stockenv={**common,'PYTHONPATH':STOCKLIB};nativeenv=lambda opt:{**common,'PYTHONPATH':EXT,'IMRE_PYTHON':CLI,'IMREFLAGS':'--opt-level '+opt}
 try:
  launcher.write_bytes(negative);(d/'gate-files.raw').write_bytes(subprocess.check_output(['sha256sum',str(launcher),str(poison)]));p,_=trace(d,'gate',['gate',str(output),*build0],nativeenv('0'));assert p.returncode==0 and not output.exists() and b'expected int, got bool' in p.stderr
 finally:launcher.write_bytes(positive)
 (d/'positive.raw').write_bytes(subprocess.check_output(['sha256sum',str(launcher)]));print(name,'gate passed',len(ids),'selected',flush=True)
 version=subprocess.check_output([PY,'-VV']).decode().strip();(d/'stock_version.raw').write_text(version+'\n');stockx,stock=capture([PY,'-B',str(launcher)],stockenv,d,'stock');assert stock['exit_code']==0 and [x['id'] for x in stockx['originals']]==ids and all(x['outcome']=='pass' for x in stockx['originals']);sc={**stock,'launcher_sha256':sha(launcher),'python_sha256':sha(PY),'version':version,'source_commit':source['commit'],'source_archive_sha256':source['sha256'],'cpython_release_tar_sha256':sha('/workspace/scratch/cpython-3.14.5.tar.gz')};dump(d/'stock.capture.json',sc);runs={'stock':stock};(d/'head.raw').write_bytes(subprocess.check_output(['git','-C','/workspace/imre-build-8571','rev-parse','HEAD','HEAD^{tree}']));(d/'clean.raw').write_bytes(subprocess.check_output(['git','-C','/workspace/imre-build-8571','status','--porcelain']));(d/'platform.raw').write_bytes(subprocess.check_output(['uname','-s','-m','-r']));(d/'cargo.raw').write_bytes(pathlib.Path('/workspace/scratch/imre-8571-build.log').read_bytes());tree=pathlib.Path(EXT)/'test';stash=tree.with_name('test.raw-control-stash');leaf=pathlib.Path(EXT)/source['path'].removeprefix('Lib/');stockleaf=pathlib.Path(STOCKLIB)/source['path'].removeprefix('Lib/');assert sha(leaf)==sha(stockleaf)==source['sha256'];images=[]
 for tier,opt in (('O0','0'),('O3','3')):
  image=str(SCR/'images'/f'{name}-{tier}.exe');argv=build0.copy();argv[argv.index('--opt-level')+1]=opt;argv[-1]=image;p,sec=cmd(argv,nativeenv(opt),str(ROOT),timeout=300);(d/f'{tier}.build.stderr').write_bytes(p.stderr);(d/f'{tier}.build.stdout').write_bytes(p.stdout);assert p.returncode==0 and pathlib.Path(image).is_file(),p.stderr[-500:];print(name,tier,'built',sec,flush=True);x,meta=capture([image],nativeenv(opt),d,tier);assert meta['exit_code']==0 and [a['id'] for a in x['originals']]==ids and all(a['outcome']=='pass' for a in x['originals']);p,secw=trace(d,tier+'.withdraw',['withdraw',str(tree),str(stash),str(leaf),image],nativeenv(opt),str(CWD));assert p.returncode==0 and tree.exists() and sha(leaf)==source['sha256'] and p.stderr==(d/f'{tier}.stderr').read_bytes();elf,_=cmd(['readelf','-d',image],timeout=30);(d/f'{tier}.readelf.stdout').write_bytes(elf.stdout);assert elf.returncode==0 and b'libpython' not in elf.stdout.lower();meta.update(build={'argv':argv,'exit_code':0,'timed_out':False,'seconds':sec},image_sha256=sha(image),withdrawal={'readelf':{'argv':['readelf','-d',image],'exit_code':0,'stdout_sha256':sha(d/f'{tier}.readelf.stdout')}});runs[tier]=meta;images.append(image);print(name,tier,'withdrawn',secw,flush=True)
 paths=[CLI,RT,PROBE,PY,str(leaf),str(stockleaf),*images];(d/'identity.raw').write_bytes(subprocess.check_output(['sha256sum',*paths]));(d/'restored.raw').write_bytes(subprocess.check_output(['sha256sum',str(leaf)]));platform={'os':'linux','arch':'x86_64','release':subprocess.check_output(['uname','-r']).decode().strip()};gate={'family':'json','command':build0,'first_party_module':str(poison),'launcher_sha256':hashlib.sha256(negative).hexdigest(),'first_party_module_sha256':sha(poison),'build':{'head':PIN,'tree':(d/'head.raw').read_text().splitlines()[1],'cli_sha256':sha(CLI),'runtime_sha256':sha(RT),'command':['cargo','build','--offline','--release','--locked','-p','imre-compiler','-p','imre-runtime','--target-dir','/workspace/imre/target','-j','2'],'result':(d/'cargo.raw').read_text().splitlines()[-1].strip()}};run={'family':'json','revision':PIN,'source':source,'platform':platform,'launcher_sha256':sha(launcher),'stock_python_sha256':sha(PY),'stock_version':version,'runs':runs};dump(d/'gate.json',gate);dump(d/'run.json',run);print(name,'complete',len(ids),flush=True)

if __name__=='__main__':
 rows=[json.loads(s)for s in (SNAP/'stdlib/json.jsonl').read_text().splitlines()];candidates=[r for r in rows if all(r['runs'][t]['outcome']=='pass' for t in ('stock','O0','O3'))];groups={};
 for r in candidates:groups.setdefault(r['source']['path'],[]).append(r)
 for src,rs in sorted(groups.items(),key=lambda v:-len(v[1])):
  name=pathlib.Path(src).stem.removeprefix('test_');name='backend' if name=='__init__' else name
  if len(sys.argv)>1 and name not in sys.argv[1:]:continue
  if (OUT/name/'run.json').exists():continue
  try:cohort(name,rs)
  except Exception as exc:
   print(name,'BLOCKED',type(exc).__name__,str(exc)[:300],flush=True)
   tree=pathlib.Path(EXT)/'test';stash=tree.with_name('test.raw-control-stash')
   if stash.exists() and not tree.exists():stash.rename(tree)
