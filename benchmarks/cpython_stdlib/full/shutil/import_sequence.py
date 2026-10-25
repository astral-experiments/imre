"""Observe the original guarded posix import and isolate the import-order requirement."""
import argparse
import builtins
import hashlib
import importlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import verify_files, verify_interpreter_tests


def observe_original(missing):
    import os
    import posix
    import test
    manifest = json.loads((HERE / 'UPSTREAM.json').read_bytes())
    verify_files(STDLIB/'upstream', manifest['files'])
    verify_interpreter_tests(manifest)
    lib = STDLIB/'upstream/Lib/test'
    test.__path__.insert(0, str(lib))
    events = []
    original = builtins.__import__

    def trace(name, globals=None, locals=None, fromlist=(), level=0):
        if (globals or {}).get('__name__') != 'test.test_shutil' or name not in {
            'posix', 'test', 'test.support', 'test.support.os_helper'}:
            return original(name, globals, locals, fromlist, level)
        event = {'line': sys._getframe(1).f_lineno, 'module': name, 'fromlist': list(fromlist or ())}
        events.append(event)
        if missing and name == 'posix':
            event['injected_error'] = 'ModuleNotFoundError'
            raise ModuleNotFoundError("No module named 'posix'", name='posix')
        result = original(name, globals, locals, fromlist, level)
        event['returned'] = result.__name__
        return result

    with patch.object(builtins, '__import__', trace):
        module = importlib.import_module('test.test_shutil')
    assert Path(module.__file__).resolve() == lib/'test_shutil.py'
    assert [(e['line'],e['module']) for e in events[:4]] == [
        (27,'posix'),(31,'test'),(32,'test.support'),(33,'test.support.os_helper')]
    assert module.posix is (None if missing else posix)
    assert missing or module.posix.getcwd() == os.getcwd()
    count = unittest.defaultTestLoader.loadTestsFromModule(module).countTestCases()
    assert count == 223
    print(json.dumps({'backend':'CPython','fixture':'injected missing posix' if missing else 'real Linux posix',
                      'events':events,'posix_is_original_module':module.posix is posix,
                      'cases_discovered':count,'tests_executed':0}))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--child', choices=['present','missing'])
    for key in ('imre','runtime','clang','result'):
        p.add_argument('--'+key, type=lambda s:Path(s).resolve())
    args = p.parse_args()
    if sys.platform != 'linux' or sys.version_info[:3] != (3,14,5):
        p.error('requires the test-enabled CPython 3.14.5 Linux reference')
    if args.child:
        observe_original(args.child == 'missing')
        return
    if not all([args.imre,args.runtime,args.clang,args.result]):
        p.error('compiler, runtime, clang and result are required')
    with args.imre.open('rb') as compiler:
        digest = hashlib.file_digest(compiler, 'sha256').hexdigest()
    report = {'cpython_commit':json.loads((HERE/'UPSTREAM.json').read_bytes())['commit'],
              'compiler_sha256':digest,
              'tests_executed':0, 'original':[], 'controls':{}}
    for mode in ('present','missing'):
        data = subprocess.check_output([sys.executable,'-B',__file__,'--child',mode],text=True)
        report['original'].append(json.loads(data))
    for mode, name in [('present','posix'),('missing','_imre_absent_optional_probe'),('raises','optional_failure')]:
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            if mode == 'raises':
                (root/(name+'.py')).write_text('print("inside optional import")\nraise ImportError("child initialization failed")\n')
            (root/'after.py').write_text('print("after import")\n')
            suffix = ('        print(target.__name__)\n        print(target.getcwd() == os.getcwd())\n'
                      if mode == 'present' else '        print("present")\n')
            consumer = (f'import os\ntry:\n    import {name} as target\nexcept ImportError:\n    target = None\n'
                        'import after\ndef observe() -> None:\n    if target is None:\n        print("absent")\n    else:\n'+suffix)
            (root/'consumer.py').write_text(consumer)
            entry = root/'main.py'
            entry.write_text('import consumer\nconsumer.observe()\n')
            expected = {'present':['after import','posix','True'], 'missing':['after import','absent'],
                        'raises':['inside optional import','after import','absent']}[mode]
            # Explicit path insertion also works with the devbox's safe-path setting.
            oracle = subprocess.run([sys.executable,'-IB','-c',"import sys;sys.path.insert(0,'.');import main"],
                                    cwd=root,capture_output=True,text=True,check=True)
            assert oracle.stdout.splitlines()==expected and not oracle.stderr
            row = {'cpython_stdout':expected,'consumer_sha256':hashlib.sha256(consumer.encode()).hexdigest(),'levels':{}}
            report['controls'][mode] = row
            for level in (0,3):
                binary = root/'program'
                build = subprocess.run([args.imre,'build',entry,'--module-root',root,'--opt-level',str(level),
                                        '--no-cache','--runtime',args.runtime,'--clang',args.clang,'-o',binary],
                                       cwd=root,capture_output=True,text=True,timeout=120)
                if build.returncode:
                    row['levels'][level] = {'status':'compile-blocked','diagnostic':build.stderr.strip().splitlines()[-1].replace(str(root)+'/','')}
                    continue
                actual = subprocess.run([binary],cwd=root,capture_output=True,text=True,timeout=30)
                row['levels'][level] = {'status':'match' if (actual.returncode,actual.stdout,actual.stderr)==(0,oracle.stdout,'') else 'failed',
                                        'exit':actual.returncode,'stdout':actual.stdout.splitlines(),'stderr':actual.stderr}
    args.result.write_text(json.dumps(report,indent=2)+'\n')
    return int(any(r['status']=='failed' for row in report['controls'].values() for r in row['levels'].values()))


if __name__ == '__main__':
    sys.exit(main())
