"""Test each selected original method even when an unrelated stream operation is unavailable."""
import argparse
import ast
from collections import Counter
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from native import (BLOB, CASES, HERE, HOST_CASES, SOURCE, Toolchain, fixture, host_executor,
                    load_suite, native_method, run_in_fixtures, verify_files, verify_interpreter_tests)
from suite_inventory import cases
from suite_native import NativeCompileError, compile_blocker, method_bytes


def driver_for(operations):
    # Only narrow the commands of our test driver. The upstream class, method,
    # arguments, assertions, and operations executed by each test are untouched.
    class Commands(ast.NodeTransformer):
        def visit_If(self, node):
            node = self.generic_visit(node)
            if isinstance(node.test, ast.Compare) and isinstance(node.test.left, ast.Name) and node.test.left.id == 'command':
                assert len(node.test.comparators) == 1 and isinstance(node.test.ops[0], ast.Eq)
                name = node.test.comparators[0].value
                if name != 'stop' and name not in operations:
                    return node.orelse
            return node
    return (ast.unparse(Commands().visit(ast.parse((HERE/'driver.py').read_bytes())))+'\n').encode()


def transcript(toolchain, root, name, source):
    folder = root/name; folder.mkdir()
    try:
        binary = toolchain.build(source, folder)
    except NativeCompileError as error:
        return compile_blocker(error)
    oracle = subprocess.run([sys.executable, '-B', folder/'case.py'], capture_output=True, timeout=30)
    native = subprocess.run([binary], capture_output=True, timeout=30)
    assert oracle.returncode == 0 and not oracle.stderr
    if (native.returncode, native.stdout, native.stderr) != (0, oracle.stdout, oracle.stderr):
        return {'outcome':'failed', 'returncode':native.returncode, 'stdout':native.stdout.decode(), 'stderr':native.stderr.decode(), 'reference':oracle.stdout.decode()}
    return {'outcome':'matched', 'native_stdout_sha256':hashlib.sha256(native.stdout).hexdigest()}


def main():
    p=argparse.ArgumentParser(description=__doc__)
    for opt in ('imre','runtime','clang','cpython-source','result'):
        p.add_argument('--'+opt,type=Path,required=True)
    a=p.parse_args(); assert sys.version_info[:3] == (3,14,5)
    manifest=json.loads((HERE.parent/'UPSTREAM.json').read_bytes()); verify_files(HERE.parent/'upstream',manifest['files'])
    verify_files(a.cpython_source,{SOURCE:BLOB}); sys.path.insert(0,str(a.cpython_source.resolve()/'Lib'))
    verify_interpreter_tests({'files':{SOURCE:BLOB}}); source=a.cpython_source.resolve()/SOURCE
    history=json.loads((HERE/'observations-linux.json').read_text())['observations']
    known={c.id() for c in cases(load_suite('test.test_memoryio')) if c.id().startswith('test.test_memoryio.CStringIOTest.')}; assert len(known)==39
    report={'cpython_commit':manifest['commit'],'original_class_denominator':len(known),'selected':len(CASES)+len(HOST_CASES),'observations':{}}
    with tempfile.TemporaryDirectory() as scratch:
        for level in (0,3):
            t=Toolchain(a.imre,a.runtime,a.clang,level);root=Path(scratch)/str(level);root.mkdir();rows={}; controls={}
            for method,owner in {**CASES,**HOST_CASES}.items():
                key='test.test_memoryio.CStringIOTest.'+method; assert key in known
                baseline=history[f'O{level}'][key]['evidence']; assert hashlib.sha256(method_bytes(source,owner,method)).hexdigest()==baseline['method_sha256']
                folder=root/method;folder.mkdir()
                try:
                    if method in CASES:
                        execute=native_method(t,folder,source,owner,key,fixture(method),operation=('imre','native io.StringIO'))
                    else:
                        wanted=baseline['native_calls']; binary=t.build(driver_for(set(wanted)-{'StringIO'}),folder)
                        execute=host_executor(binary,source,owner,method,level)
                    evidence,failures,skips=run_in_fixtures(load_suite('test.test_memoryio'),{key:execute})
                    if failures or skips:
                        rows[key]={'outcome':'failed','failures':failures,'skips':skips}
                    else:
                        obs=evidence[key][0]['observation']; assert obs['roles']['operation']=='imre'
                        if method in HOST_CASES: assert obs['evidence']['native_calls']==baseline['native_calls']
                        rows[key]={'outcome':'passed',**obs}
                except NativeCompileError as error: rows[key]=compile_blocker(error)
                print(f'O{level}: {method}: {rows[key]["outcome"]}',flush=True)
            for name,src in {'all_streams':(HERE/'streams.py').read_bytes(),
                'csv_shared':b'from io import StringIO\nimport csv\nf=StringIO(\'"a\\nb",c\\r\\nd,e\\n\', newline="")\nrows=csv.reader(f)\nprint(repr(next(rows)))\nprint(repr(f.readline()))\nf.seek(0)\nprint(repr(next(rows)))\n',
                'json_shared':b'from io import StringIO\nimport json\nf=StringIO()\njson.dump([1, 2],f)\nprint(f.getvalue())\nf.seek(0)\nprint(json.dumps(json.load(f)))\n'}.items():
                controls[name]=transcript(t,root,'control_'+name,src)
            report['observations'][f'O{level}']={'methods':rows,'controls':controls,'counts':dict(Counter(row['outcome'] for row in rows.values()))}
        # Normalize only temporary paths, without changing diagnostics or outcomes.
        raw=json.dumps(report,sort_keys=True).replace(scratch,'<scratch>'); report=json.loads(raw)
    a.result.write_text(json.dumps(report,indent=2,sort_keys=True)+'\n')
    print('STRINGIO_869_RECEIPT='+json.dumps(report,sort_keys=True,separators=(',',':')))


if __name__=='__main__':main()
