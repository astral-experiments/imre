"""Verification branch only: test a clean product tree and preserve its exact inputs."""

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path.cwd()
suite = root / 'benchmarks/cpython_stdlib/full/collections'
out = Path(sys.argv[1])
out.mkdir()
imre = root / 'target/debug/imre'
runtime = root / 'target/debug/libimre_runtime.a'
clang = '/usr/bin/clang-18'


def git(spec):
    return subprocess.check_output(['git', 'rev-parse', spec], text=True).strip()


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def fixture(rel, level, reachable=False):
    src = root / 'tests/fixtures' / rel
    with tempfile.TemporaryDirectory() as temp:
        temp = Path(temp)
        native, reference = temp / 'native', temp / 'reference'
        native.mkdir()
        reference.mkdir()
        expected = subprocess.run([sys.executable, '-E', '-B', src], cwd=reference, capture_output=True, check=True)
        binary = temp / 'case'
        modules = ['--module-root', src.parent] if src.name == 'main.py' else []
        subprocess.run([imre, 'build', src, *modules, '--runtime', runtime, '--clang', clang,
                        '--opt-level', str(level), *(['--reachable-only'] if reachable else []), '--no-cache', '-o', binary], cwd=temp, check=True)
        actual = subprocess.run([binary], cwd=native, capture_output=True, check=True)
        if (actual.stdout, actual.stderr) != (expected.stdout, expected.stderr):
            raise AssertionError((rel, level, actual.stdout, expected.stdout, actual.stderr, expected.stderr))
        def files(directory):
            return {str(p.relative_to(directory)): digest(p) for p in directory.rglob('*') if p.is_file()}
        assert files(native) == files(reference), (rel, level)
        return {'lines': len(expected.stdout.splitlines()), 'stdout_sha256': hashlib.sha256(actual.stdout).hexdigest()}


def write(name, value):
    (out / name).write_text(json.dumps(value, indent=2) + '\n')


def load(path):
    return json.loads(path.read_text())


revision = git('HEAD')
write('source.json', {'compiler_revision': revision, 'tree': git('HEAD^{tree}'), 'base': git('HEAD^1'),
                      'head': git('HEAD^2'), 'imre_sha256': digest(imre), 'runtime_sha256': digest(runtime),
                      'cpython': sys.version, 'cpython_commit': '5607950ef232dad16d75c0cf53101d9649d89115'})
for level in (0, 3):
    common = ['--imre', imre, '--runtime', runtime, '--clang', clang, '--compiler-revision', revision,
              '--opt-level', str(level)]
    for runner in ('routines', 'doctests', 'counter_host', 'defaultdict_factory_host'):
        path = out / f'{runner}-O{level}.json'
        code = subprocess.run([sys.executable, suite / f'{runner}.py', *common, '--result', path]).returncode
        data = load(path)
        assert (data['compiler_revision'], data['opt_level']) == (revision, level)
        if runner == 'doctests':
            assert code in (0, 1) and len(data['tests']) == 16
            assert all(t['status'] in ('compile-blocked', 'native-body/host-output-check') for t in data['tests'].values())
            assert (code == 0) == all(t['status'] == 'native-body/host-output-check' for t in data['tests'].values())
        else:
            assert code == 0, (runner, code)
        if runner == 'routines':
            rows = data['routines']
            assert len(rows) == data['reference_denominator'] == 659
            assert len([k for k in rows if k.startswith('test.test_deque.')]) == 81
            assert not {k: d for k, d in rows.items() if d['result'] in ('failed', 'fixture-skip')}
        if runner == 'deque_methods':
            assert len(data['tests']) == 4 and not (data['failures'] or data['skips'])
    focus = {rel: fixture(rel, level) for rel in ('containers/deque_reinitialize.py', 'containers/deque.py', 'containers/chainmap.py', 'containers/namedtuple.py', 'module_values/namedtuple/main.py', 'bytes/membership.py', 'text/percent_scalar.py', 'processes/file_redirects.py', 'json_records/stdlib_deep_decode.py')}
    write(f'fixtures-O{level}.json', focus)
subprocess.run([sys.executable, suite / 'native_report.py',
                '--routines', out / 'routines-O0.json', out / 'routines-O3.json',
                '--doctests', out / 'doctests-O0.json', out / 'doctests-O3.json',
                '--host', out / 'counter_host-O0.json', out / 'counter_host-O3.json',
                '--host', out / 'defaultdict_factory_host-O0.json', out / 'defaultdict_factory_host-O3.json',
                '--result', out / 'evidence-linux.json'], check=True)
subprocess.run([sys.executable, suite.parents[1] / 'suite_evidence.py', suite / 'reference-linux.json',
                out / 'evidence-linux.json'], check=True)

subprocess.run([sys.executable, root / 'benchmarks/cpython_stdlib/full/types/run.py',
                '--imre', imre, '--runtime', runtime, '--clang', clang, '--cpython-source', root / '.cpython-collections',
                '--result', out / 'types-integration.json'], check=True)
with (out / 'file-ownership.txt').open('w') as log:
    subprocess.run([sys.executable, root / 'tests/files.py', '--compiler', imre, '--runtime', runtime,
                    '--clang', clang, '--opt-levels', '0', '3'], stdout=log, stderr=subprocess.STDOUT, check=True)

write('product-four-modes.json',{p:{f'O{o}-reachable-{int(r)}': fixture(p,o,r) for o in (0,3) for r in (False,True)} for p in ['containers/deque_reinitialize.py', 'containers/deque.py', 'containers/chainmap.py']})

import os
with tempfile.TemporaryDirectory() as dedicated:
    check=Path(dedicated)/'basics.py'
    check.write_text('"""Check the unchanged method, optionally link a no-op reverse into a second executable."""\nimport hashlib,json,os,subprocess,sys,tempfile\nfrom pathlib import Path\nroot=Path.cwd();suite=root/\'benchmarks/cpython_stdlib/full/collections\';sys.path.insert(0,str(suite))\nfrom routines import native_fixture,failure\nfrom suite_native import Toolchain,native_method,run_in_fixtures\nfrom suite_inventory import load_suite,verify_files\nfrom test import test_deque\ncase_id=\'test.test_deque.TestBasic.test_basics\'\ncase=test_deque.TestBasic(\'test_basics\')\npath=suite/\'upstream/Lib/test/test_deque.py\'\nmanifest=json.loads((suite/\'UPSTREAM.json\').read_bytes());verify_files(suite/\'upstream\',manifest[\'files\'])\nverify_files(Path(test_deque.__file__).resolve().parents[2],manifest[\'files\'])\nnode,prefix=native_fixture(path,\'TestBasic\',\'test_basics\',case)\ntarget=Path(os.environ[\'IMRE_CHECKED_TARGET\'])\nreports=[]\nrevision,mode,destination=sys.argv[1:]\nfor level in (0,3):\n    with tempfile.TemporaryDirectory(prefix=\'imre-collections-deque-basics-\') as temp:\n        d=Path(temp)\n        tools=Toolchain(target/\'imre\',target/\'libimre_runtime.a\',Path(os.environ[\'IMRE_CHECKED_CLANG\']),level)\n        try:\n            callback=native_method(tools,d,path,\'TestBasic\',case_id,prefix,operation=(\'imre\',\'Reinitialize, grow, pop, iterate, and compare native deque contents; reverse the list of right pops.\'))\n        except (ValueError,RuntimeError) as e:\n            row=failure(e,d,node,prefix)\n            if mode!=\'blocked\' or row.get(\'location\')!=\'upstream:47:9\':raise\n            reports.append(dict(opt_level=level,**row))\n            print(level,row,flush=True);continue\n        assert mode==\'supported\'\n        execution,failed,skips=run_in_fixtures(load_suite(\'test.test_deque\'),{case_id:callback})\n        assert not failed and not skips and list(execution)==[case_id],(failed,skips)\n        # The method bytes, bundled unittest assertions and all other operations stay identical.\n        (d/\'noop.c\').write_text(\'void __wrap_imre_list_reverse(void *root) {(void)root;}\\n\')\n        negative=d/\'case-noop\'\n        command=[tools.imre,\'build\',d/\'case.py\',\'--no-cache\',\'--runtime\',tools.runtime,\'--clang\',tools.clang,\'--opt-level\',str(level),\'-C\',\'link-arg=-Wl,--wrap=imre_list_reverse\',\'-C\',\'link-arg=\'+str(d/\'noop.c\'),\'-o\',negative]\n        subprocess.run(command,check=True,capture_output=True)\n        wrong=subprocess.run([negative],capture_output=True,text=True)\n        assert wrong.returncode and \'AssertionError\' in wrong.stderr and \'completed \'+case_id not in wrong.stdout,(wrong.returncode,wrong.stdout,wrong.stderr)\n        reports.append(dict(opt_level=level,execution=execution[case_id],negative=dict(exit=wrong.returncode,stderr=wrong.stderr.replace(temp,\'<work>\'),operation=\'link imre_list_reverse to a no-op\')))\n        print(level,\'method and negative control passed\',flush=True)\nPath(destination).write_text(json.dumps(dict(cpython_commit=manifest[\'commit\'],compiler_revision=revision,imre_sha256=hashlib.file_digest((target/\'imre\').open(\'rb\'),\'sha256\').hexdigest(),runtime_sha256=hashlib.file_digest((target/\'libimre_runtime.a\').open(\'rb\'),\'sha256\').hexdigest(),case=case_id,results=reports),indent=2)+\'\\n\')\n')
    subprocess.run([sys.executable,check,git('HEAD'),'blocked',out/'test-basics.json'],env=os.environ|{'IMRE_CHECKED_TARGET':str(imre.parent),'IMRE_CHECKED_CLANG':str(clang)},check=True)
