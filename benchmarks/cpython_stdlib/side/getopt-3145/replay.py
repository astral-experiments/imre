"""Replay the intact upstream test_getopt with the specified native parent and child."""
import argparse, json, os, re, shutil, threading
from pathlib import Path
from supervisor import sha, run, save
kit=Path(__file__).resolve().parent
manifest=json.loads((kit/'manifest.json').read_bytes())
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--work',type=Path,required=True)
p.add_argument('--cpython',type=Path,required=True)
p.add_argument('--pair',type=Path)
p.add_argument('--clang',type=Path)
p.add_argument('--tag',required=True)
p.add_argument('--levels',nargs='+',type=int,choices=(0,3),default=[0,3])
p.add_argument('--layouts',nargs='+',choices=('installed','wrong','embedded'),default=['installed','wrong'])
p.add_argument('--children',nargs='+',choices=('stock','imre'),default=['stock','imre'])
a=p.parse_args();w=a.work.resolve()/a.tag;w.mkdir(parents=True,exist_ok=True)
for n,h in manifest['layout'].items():assert sha(kit/'stage'/n)==h
def states(v):
 ids=[s[3:] for s in v['stdout'].splitlines() if s.startswith('ID ')]
 matches=re.findall(r'^.+ \.\.\. (ok|FAIL|ERROR|skipped[^\n]*)$',v['stderr'],re.M)
 return dict(ordered=ids==manifest['ids'],registered=len(ids),outcomes=matches,summary=v['stderr'].splitlines()[-1] if v['stderr'] else None)
def observe(stop,stage,work,observations):
 py=str(stage/'Tools/i18n/pygettext.py').encode();seen=set()
 while not stop.is_set():
  for proc in Path('/proc').glob('[0-9]*'):
   try:
    if os.readlink(proc/'cwd')!=str(work):continue
    args=(proc/'cmdline').read_bytes().split(b'\0')[:-1]
    if len(args)<3 or py not in args[1:3] or not any(s in args for s in (b'--no-location',b'--imre-invalid-location')):continue
    exe=os.readlink(proc/'exe');key=(proc.name,exe,tuple(args))
    if key in seen:continue
    seen.add(key);observations.append(dict(pid=int(proc.name),exe=exe,argv=list(map(os.fsdecode,args)),ppid=int((proc/'stat').read_text().rsplit(')',1)[1].split()[1])))
   except (FileNotFoundError,PermissionError,ProcessLookupError):pass
  stop.wait(.02)
def program(argv,cwd,limit,stage):
 seen=[];stop=threading.Event();worker=threading.Thread(target=observe,args=(stop,stage,cwd,seen));worker.start()
 try:v=run(argv,cwd,limit)
 finally:stop.set();worker.join()
 for o in seen:
  path=Path(o['exe']);o['exe_sha256']=sha(path) if path.is_file() else None
 return dict(command=v,child_processes=seen,**states(v))
cp=a.cpython.resolve()
j=dict(cpython_commit=manifest['upstream'],cpython_executable=str(cp),cpython_sha256=sha(cp),inputs=manifest['layout'],ordered_ids=manifest['ids'],rows={})
out=w/'result.json'
if out.exists():j=json.loads(out.read_bytes())
if a.pair is not None:
 pair=a.pair.resolve();clang=a.clang.absolute();assert (pair/'tree').is_file()
 products={Path(n).name:h for h,n in (l.split() for l in (pair/'pair.sha256').read_text().splitlines())}
 assert all(sha(pair/n)==h for n,h in products.items())
 r=dict(commit=(pair/'commit').read_text().strip(),tree=(pair/'tree').read_text().strip(),products=products,clang=str(clang),clang_sha256=sha(clang))
 assert 'pair' not in j or j['pair']==r;j['pair']=r
for layout in a.layouts:
 stage=w/layout/'stage'
 if not stage.is_dir():
  shutil.copytree(kit/'stage',stage)
  if layout=='embedded':(stage/'Lib/getopt.py').unlink()
  if layout=='wrong':
   h=stage/'Lib/test/support/i18n_helper.py';s=h.read_bytes();assert s.count(b"'--no-location'")==1
   h.write_bytes(s.replace(b"'--no-location'",b"'--imre-invalid-location'"))
 src=stage/'Lib/driver.py'
 results=j['rows'].setdefault(layout,dict(layout_hashes={str(f.relative_to(stage)):sha(f) for f in sorted(stage.rglob('*')) if f.is_file() and '__pycache__' not in f.parts}))
 if a.pair is None:
  if 'cpython' not in results:results['cpython']=program([cp,'-B',src,'module'],w/layout/'stock-run',180,stage)
  save(out,j);print(layout,'cpython',results['cpython']['command']['exit'],results['cpython']['outcomes'],flush=True);continue
 for level in a.levels:
  key='O'+str(level);buildroot=w/layout/key;elf=buildroot/'original'
  if key in results and all(t in results[key] for t in a.children):continue
  if key not in results or not elf.is_file():
   if key in results:raise RuntimeError('selected evidence is sealed; use a new --tag')
   cmd=[pair/'imre','build',src,'--module-root',src.parent,'--runtime-imports','--no-cache','--runtime',pair/'libimre_runtime.a','--opt-level',level,'--clang',clang,'-o',elf]
   b=run(cmd,buildroot/'build',600);row=dict(build=b);results[key]=row;save(out,j)
   if b['exit']!=0:print(layout,key,'build',b['exit'],b['stderr'][-250:],flush=True);continue
   row['elf_sha256']=sha(elf)
  row=results[key]
  for child in a.children:
   if child in row:continue
   launcher=cp if child=='stock' else pair/'imre'
   flags=['--no-cache','--runtime',str(pair/'libimre_runtime.a'),'--clang',str(clang),'--opt-level',str(level)]
   argv=['/usr/bin/env','IMRE_PYTHON='+str(launcher),'IMRE_ENCODED_FLAGS='+'\x1f'.join(flags),elf,'module']
   row[child]=program(argv,buildroot/(child+'-run'),240,stage);save(out,j)
   v=row[child];print(a.tag,layout,key,child,v['command']['exit'],v['outcomes'],v['summary'],len(v['child_processes']),flush=True)
  elf.unlink(missing_ok=True)
save(out,j)
