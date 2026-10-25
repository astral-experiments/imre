import ast, hashlib, json, os, re, subprocess, sys, tempfile
from pathlib import Path
runner_temp=Path(os.environ['RUNNER_TEMP']); info=json.loads((runner_temp/'subprocess-finalizer-source.json').read_text()); product=Path(os.environ['CARGO_TARGET_DIR'])/'debug'; py=sys.executable
source=Path('tests/fixtures/processes/unobserved_child.py').read_bytes(); test=Path('tests/processes.py').read_text()
func=next(n for n in ast.parse(test).body if isinstance(n,ast.FunctionDef) and n.name=='unobserved_children'); namespace={'re':re}; exec(compile(ast.Module(body=[func],type_ignores=[]),'<upstream test helper>','exec'),namespace); check=namespace['unobserved_children']
def record(result):
    check(result); lines=result.stdout.splitlines(); pids={k.decode():int(v) for k,v in zip(lines[:-1:2],lines[1:-1:2],strict=True)}; emitted=list(map(int,re.findall(rb'ResourceWarning: subprocess (\d+) is still running',result.stderr))); return dict(pids=pids,emitted=emitted)
observed={}; digest=lambda p: hashlib.file_digest(p.open('rb'),'sha256').hexdigest()
with tempfile.TemporaryDirectory() as td:
    temp=Path(td); case=temp/'unobserved_child.py'; case.write_bytes(source); observed['oracle']=record(subprocess.run([py,'-E','-B',case],capture_output=True))
    for level in (0,3):
        binary=temp/f'check-{level}'; subprocess.run([product/'imre','build',case,'--no-cache','--runtime',product/'libimre_runtime.a','--clang','/usr/bin/clang-18','--opt-level',str(level),'-o',binary],check=True,capture_output=True)
        prefix=temp/f'trace{level}'; result=subprocess.run(['strace','-ff','-qq','-s','256','-e','signal=none','-e','trace=execve,clone,clone3,fork,vfork,wait4,waitid,write','-o',prefix,'--',binary],capture_output=True,timeout=30); row=record(result)
        traces={int(f.suffix[1:]):f.read_text().splitlines() for f in temp.glob(prefix.name+'.*')}; [parent]=[pid for pid,lines in traces.items() if any(s.startswith('execve('+json.dumps(str(binary))+',') and s.endswith('= 0') for s in lines)]
        for pid in row['pids'].values():
            assert any(re.match(r'(clone3?|vfork|fork)\(',s) and s.endswith(f'= {pid}') for s in traces[parent])
            assert any(s.startswith('execve("/bin/sh",') and s.endswith('= 0') for s in traces[pid])
        assert len(set(row['pids'].values()))==7
        row.update(native_parent=parent,actor_sha256=digest(binary)); observed[str(level)]=row
report=dict(**info,toolchain={n:digest(product/f) for n,f in [('imre','imre'),('runtime','libimre_runtime.a')]},fixture_sha256=hashlib.sha256(source).hexdigest(),observations=observed)
(runner_temp/'subprocess-finalizer-verified.json').write_text(json.dumps(report,indent=2)+'\n'); print([(k,v['emitted']) for k,v in observed.items()])
