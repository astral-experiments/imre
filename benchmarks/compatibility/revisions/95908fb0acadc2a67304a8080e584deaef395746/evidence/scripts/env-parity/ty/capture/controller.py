from pathlib import Path
import hashlib,json,subprocess,time,shutil,os
BASE=Path('/workspace/ruff-b-practical-95908/followup-880648'); OLD=BASE/'ty05'; CAP=Path('/workspace/scratch/ty05-959-raw-review-v1'); OUT=Path('/workspace/scratch/ty05-959-env-parity-v1'); OUT.mkdir()
OLD_SOURCE=OLD/'fixture/scripts/transform_readme.py'; REF=json.loads((OLD/'report.json').read_text()); REV=REF['archived_checkout']; SUP=CAP/'supervisor'; BWRAP=Path('/usr/bin/bwrap'); ENV=Path('/usr/bin/env')
def sha(p):return hashlib.file_digest(p.open('rb'),'sha256').hexdigest()
def git(f):return subprocess.check_output(['git','-C',str(BASE/'ty-source'),'show',REV+':'+f])
def snap(d):return {str(p.relative_to(d)):{'size':p.stat().st_size,'sha256':sha(p)} for p in d.rglob('*') if p.is_file()}
trace_set='execve,execveat,open,openat,openat2,creat,io_uring_setup,clone,clone3,fork,vfork,rename,renameat,renameat2,newfstatat,statx,wait4,waitid,exit,exit_group,chdir,fchdir,close,close_range,dup,dup2,dup3,fcntl,unshare,mount,umount2,pivot_root,mount_setattr,open_tree,move_mount'
mask=['--ro-bind','/','/','--tmpfs',str(OLD_SOURCE.parent)]
assert OLD_SOURCE.is_file()
smoke_cmd=['/usr/bin/strace','-f','-v','-yy','-qq','-s','4096','-e','trace='+trace_set,'-o',str(OUT/'mask-smoke.trace'),str(BWRAP),*mask,'--','/usr/bin/stat',str(OLD_SOURCE)]
s=subprocess.run(smoke_cmd,capture_output=True,timeout=10,env={'PATH':'/usr/bin:/bin','LC_ALL':'C.UTF-8'}); assert s.returncode==1 and b'No such file or directory' in s.stderr and OLD_SOURCE.is_file()
(OUT/'mask-smoke.json').write_text(json.dumps({'argv':smoke_cmd,'exit_code':s.returncode,'stdout_sha256':hashlib.sha256(s.stdout).hexdigest(),'stderr':s.stderr.decode(),'trace_sha256':sha(OUT/'mask-smoke.trace'),'parent_source_exists_after':OLD_SOURCE.is_file(),'bwrap_sha256':sha(BWRAP)},indent=2,sort_keys=True)+'\n')
results={}
for tier in ('O0','O3'):
 d=OUT/('mask-'+tier);d.mkdir();root=d/'fixture';(root/'scripts').mkdir(parents=True);launch=d/'launch';launch.mkdir()
 (root/'README.md').write_bytes(git('README.md'));(root/'scripts/transform_readme.py').write_bytes(git('scripts/transform_readme.py'));shutil.copyfile(BASE.parent/'ty05-fixture/dist-workspace.toml',root/'dist-workspace.toml')
 pre=snap(root);assert {k:v['sha256'] for k,v in pre.items()}==REF['preimages']
 entry=root/'scripts/transform_readme.py';stash=d/'withdrawn.source';image=CAP/tier/'saved.exe';oldrun=json.loads((OLD/'runs'/tier/'result.json').read_text());env={k:(str(launch) if k=='HOME' else v) for k,v in oldrun['environment'].items()};trace=d/'run.trace'
 args=[str(BWRAP),*mask,'--bind',str(root),str(root),'--chdir',str(root),'--die-with-parent','--',str(ENV),'-u','PWD',str(image)]
 cmd=['/usr/bin/strace','-f','-v','-yy','-qq','-s','4096','-e','trace='+trace_set,'-o',str(trace),str(SUP),str(entry),str(stash),'-','-',str(BWRAP),*args]
 begin=time.monotonic();p=subprocess.run(cmd,cwd=root,env=env,capture_output=True,timeout=30);seconds=time.monotonic()-begin
 (d/'stdout').write_bytes(p.stdout);(d/'stderr').write_bytes(p.stderr);post=snap(root);fx={k:{'after':{'bytes':post[k]['size'],'sha256':post[k]['sha256']} if k in post else None,'before':{'bytes':pre[k]['size'],'sha256':pre[k]['sha256']} if k in pre else None} for k in sorted(pre.keys()|post.keys()) if pre.get(k)!=post.get(k)};effect=json.dumps(fx,sort_keys=True,separators=(',',':'));(d/'effects.json').write_text(effect)
 result={'argv':cmd,'cwd':str(root),'environment':env,'exit_code':p.returncode,'seconds':round(seconds,4),'image_sha256':sha(image),'before':pre,'after':post,'stdout_sha256':sha(d/'stdout'),'stderr_sha256':sha(d/'stderr'),'effects_sha256':sha(d/'effects.json'),'trace_sha256':sha(trace),'trace_bytes':trace.stat().st_size,'scratch_source_restored':entry.is_file(),'protected_parent_source_intact':OLD_SOURCE.is_file(),'protected_compile_source_masked_by_child_tmpfs':str(OLD_SOURCE.parent),'bwrap_sha256':sha(BWRAP),'supervisor_sha256':sha(SUP)}
 lines=trace.read_text().splitlines(); image_line=next(x for x in lines if 'execve('+json.dumps(str(image))+',' in x); values=json.loads(image_line.rsplit('], ',1)[1].removesuffix(') = 0')); actual=dict(x.split('=',1) for x in values); assert actual==env and 'PWD' not in actual; result.update({'actual_image_environment':actual,'env_trampoline_sha256':sha(ENV),'controller_sha256':sha(Path(__file__))})
 (d/'result.json').write_text(json.dumps(result,indent=2,sort_keys=True)+'\n');results[tier]=result
 assert p.returncode==0 and result['stdout_sha256']==oldrun['stdout_sha256'] and result['stderr_sha256']==oldrun['stderr_sha256'] and result['effects_sha256']==oldrun['effects_sha256']
(OUT/'mask-summary.json').write_text(json.dumps(results,indent=2,sort_keys=True)+'\n');print(json.dumps({k:{x:v[x] for x in ('exit_code','effects_sha256','trace_bytes','trace_sha256','image_sha256','protected_parent_source_intact')} for k,v in results.items()}))
