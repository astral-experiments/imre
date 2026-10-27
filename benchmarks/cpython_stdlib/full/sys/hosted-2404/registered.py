"""Compare the original CLI fixtures touched by the new main and the I/O consumer."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

b = Path(__file__).resolve().parent
repo = Path(os.environ['GITHUB_WORKSPACE'])
source = sys.argv[1]
pair = json.loads((b/'pair.json').read_bytes())
assert sys.version_info[:3] == (3,14,5) and pair['source'] == source
names = ('boxed_oserror', 'boxed_text_io_wrapper', 'boxed_dict_subclasses', 'boxed_itemgetter', 'boxed_deque', 'boxed_bytes_io', 'boxed_sys_intern', 'boxed_source_exec', 'boxed_codecs', 'boxed_descriptors', 'boxed_recursive_repr', 'boxed_type_identity', 'boxed_class_attribute', 'boxed_exception_repr')
def digest(p):
    with p.open('rb') as f:
        return hashlib.file_digest(f,'sha256').hexdigest()
def run(cmd, work, native=False):
    env = {'PATH':'/no-host', 'LC_ALL':'C.UTF-8'} if native else {**os.environ, 'LC_ALL':'C.UTF-8'}
    r = subprocess.run(cmd,cwd=work,capture_output=True,text=True,env=env,timeout=240)
    return {'exit':r.returncode, 'stdout':r.stdout.replace(str(work),'<work>'), 'stderr':r.stderr.replace(str(work),'<work>')}
assert all(digest(Path(path)) == pair['sha256'][k] for k,path in pair['paths'].items())
report = {'pair':pair, 'reference':sys.version, 'fixtures':{}, 'levels':{}}
with tempfile.TemporaryDirectory(prefix='sys-oserror-io-') as t:
    work = Path(t)
    entry = work/'main.py'
    entry.write_text('import sys\n'+''.join(f'if sys.argv[1] == "{name}":\n    import {name}\n    {name}.current()\n' for name in names))
    report['entry_sha256'] = digest(entry)
    for name in names:
        path=work/(name+'.py');path.write_bytes(subprocess.check_output(['git','-C',repo,'show',source+':tests/fixtures/'+path.name]))
        result=run([sys.executable,'-EB',entry,name],work);assert result['exit']==0 and not result['stderr'],(name,result)
        report['fixtures'][name]={'source_sha256':digest(path),'oracle':result}
    for opt in ('0','3'):
        elf=work/('standalone'+opt);ir=work/('module'+opt+'.ll');build=run([pair['paths']['imre'],'build',entry,'--runtime-imports','--module-root',work,'--runtime',pair['paths']['libimre_runtime.a'],'--no-cache','--clang','/usr/bin/clang-18','--opt-level',opt,'--emit-llvm',ir,'-o',elf],work)
        row=report['levels'][opt]={'build':build};assert build['exit']==0,build
        llvm=ir.read_text();row.update(elf_sha256=digest(elf), ir_sha256=digest(ir), registered=llvm.count('call void @imre_dyn_register('), typed_intern=llvm.count('call ptr @imre_sys_intern('), cases={})
        assert row['registered']>0 and row['typed_intern']==0
        for name in names:
            native=run([elf,name],work,True); item=row['cases'][name]={'native':native,'match':native==report['fixtures'][name]['oracle']}
            print(opt,name,item['match'],(native['stderr'].splitlines() or [''])[-1],flush=True)
        (b/f'registered-oserror-{source}.json').write_text(json.dumps(report,indent=2)+'\n')
