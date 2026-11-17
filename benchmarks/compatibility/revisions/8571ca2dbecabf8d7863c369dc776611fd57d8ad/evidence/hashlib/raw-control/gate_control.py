import hashlib,json,os,pathlib,subprocess,sys
R=pathlib.Path('/workspace/imre-hashlib-strict'); P='8571ca2dbecabf8d7863c369dc776611fd57d8ad'; E=R/'benchmarks/compatibility/revisions'/P/'evidence/hashlib/raw-control'; S=pathlib.Path('/workspace/scratch/hashlib-strict'); C='/workspace/imre/target/release/imre'; RT='/workspace/imre/target/release/libimre_runtime.a'; CL='/workspace/.toolchains/imre-x86_64-unknown-linux-gnu/imre-toolchain/bin/clang'; L=E/'launcher.py'; NEG=E/'gate.launcher.py'; Z=E/'hashlib_poison.py'; image=S/'gate.exe'; probe=S/'raw-control-probe'; sha=lambda b:hashlib.sha256(b).hexdigest()
env={'PYTHONPATH':str(S/'fixtures/Lib'),'PYTHONDONTWRITEBYTECODE':'1','PYTHON_COLORS':'0','NO_COLOR':'1','IMRE_PYTHON':C,'IMREFLAGS':'--opt-level 0','PATH':str(pathlib.Path(CL).parent)+':'+os.environ['PATH'],'TMPDIR':str(S/'tmp')}
command=[C,'build',str(L),'--module-root',str(E),'--extension-path',str(S/'fixtures/Lib'),'--no-cache','--quiet','--opt-level','0','--runtime',RT,'--clang',CL,'--include-module','test.test_hashlib','-o',str(image)]
trace=['strace','-f','-qq','-s','512','-e','trace=execve,execveat,open,openat,openat2,creat,io_uring_setup,newfstatat,rename,wait4,exit_group','-o',str(E/'gate.trace'),str(probe),'gate',str(image),*command]
assert not image.exists(); positive=L.read_bytes()
try:
 L.write_bytes(NEG.read_bytes()); (E/'gate-files.raw').write_text(subprocess.check_output(['sha256sum',str(L),str(Z)],text=True));
 with (E/'gate.stdout').open('wb') as out,(E/'gate.stderr').open('wb') as err: q=subprocess.run(trace,cwd=R,env={**os.environ,**env},stdout=out,stderr=err,timeout=180)
finally:
 L.write_bytes(positive); (E/'positive.raw').write_text(subprocess.check_output(['sha256sum',str(L)],text=True))
meta={'family':'hashlib','command':command,'environment':env,'launcher_sha256':sha(NEG.read_bytes()),'first_party_module':str(Z),'first_party_module_sha256':sha(Z.read_bytes()),'supervisor_exit':q.returncode,'image_exists':image.exists()}; (E/'gate.partial.json').write_text(json.dumps(meta,indent=2)+'\n'); print('supervisor_exit',q.returncode,'image_exists',image.exists()); print((E/'gate.stderr').read_text()[:1200]); sys.exit(0 if q.returncode==0 and not image.exists() and 'expected int, got bool' in (E/'gate.stderr').read_text() else 1)
