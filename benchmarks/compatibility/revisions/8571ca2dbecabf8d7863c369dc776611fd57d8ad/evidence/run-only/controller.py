"""Run-only, timeboxed recapture of four preserved 8571 snapshot images."""
import hashlib,json,os,pathlib,signal,subprocess,time
ROOT=pathlib.Path(__file__).resolve().parents[6]; HERE=pathlib.Path(__file__).resolve().parent; SNAP=HERE.parents[1]
SYSCALLS='execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,waitid,exit,exit_group,chdir,fchdir,close,close_range,dup,dup2,dup3,fcntl,clone,clone3,fork,vfork'
sha=lambda p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
controller_sha=sha(__file__);archive=HERE/'archive.json'; assert archive.is_file()
for family in ('glob','logging'):
 original=SNAP/'evidence'/family/'strict';run=json.loads((original/'run.json').read_text());binding=json.loads((original/'binding.json').read_text())['cohorts']['finite-native']
 tree=pathlib.Path(run['runs']['O0']['environment']['PYTHONPATH'])/'test';stash=pathlib.Path(str(tree)+'.raw-control-stash');leaf=tree/pathlib.Path(run['source']['path']).name
 for tier in ('O0','O3'):
  meta=run['runs'][tier];image=meta['argv'][0];expected=meta['image_sha256'];assert sha(image)==expected and tree.is_dir() and leaf.is_file() and not stash.exists()
  dest=HERE/family/tier;dest.mkdir(parents=True,exist_ok=False);env=meta['withdrawal']['environment'];probe=binding['probe'];cmd=['/usr/bin/strace','-f','-y','-qq','-s','512','-e','trace='+SYSCALLS,'-o',str(dest/'trace.raw'),probe,'withdraw',str(tree),str(stash),str(leaf),image]
  start=time.monotonic();p=subprocess.Popen(cmd,cwd=meta['cwd'],env={**os.environ,**env},stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)
  try:out,err=p.communicate(timeout=30)
  except subprocess.TimeoutExpired:
   os.killpg(p.pid,signal.SIGKILL);out,err=p.communicate()
   for source in reversed(binding['withdraw_sources']):
    path=pathlib.Path(source);saved=pathlib.Path(source+'.raw-control-stash')
    if saved.exists() and not path.exists():saved.rename(path)
   if stash.exists() and not tree.exists():stash.rename(tree)
   raise
  elapsed=round(time.monotonic()-start,3);(dest/'stdout.raw').write_bytes(out);(dest/'stderr.raw').write_bytes(err);post=sha(image);same=err==(original/(tier+'.stderr')).read_bytes() and out==(original/(tier+'.stdout')).read_bytes()
  result={'schema':'imre.compatibility.run-only.v1','family':family,'tier':tier,'controller_sha256':controller_sha,'archive_sha256':sha(archive),'argv':cmd,'cwd':meta['cwd'],'environment':env,'timeout_seconds':30,'seconds':elapsed,'exit_code':p.returncode,'image_sha256_before':expected,'image_sha256_after':post,'stdout_sha256':hashlib.sha256(out).hexdigest(),'stderr_sha256':hashlib.sha256(err).hexdigest(),'trace_sha256':sha(dest/'trace.raw'),'original_output_equal':same,'restored_source_sha256':sha(leaf),'restored_extra_sha256':{x:sha(x) for x in binding['withdraw_sources']}}
  (dest/'run.json').write_text(json.dumps(result,indent=2)+'\n');print(family,tier,'seconds',elapsed,'exit',p.returncode,'output_equal',same,'image_equal',post==expected,flush=True)
  assert p.returncode==0 and same and post==expected and not stash.exists()
