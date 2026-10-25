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


def fixture(rel, level):
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
                        '--opt-level', str(level), '--no-cache', '-o', binary], cwd=temp, check=True)
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
source = root / '.cpython-collections'
subprocess.run([sys.executable, '-E', '-B', suite / 'theme_section.py',
                '--cpython-source', source, '--imre', imre, '--runtime', runtime, '--clang', clang,
                '--compiler-revision', revision, '--result', out / 'theme-section.json'], check=True)
startup = subprocess.run([sys.executable, '-B', suite.parent / 'check_suite_startup.py',
                          '--compiler', imre, '--cpython-git', source, '--support-file', '_colorize.py',
                          'test.test_collections', 'test.test_argparse'], capture_output=True, text=True)
if startup.returncode not in (0, 1): raise RuntimeError(startup.stderr)
data = json.loads(startup.stdout)
assert len(data['checks']) == 8 and data['tests_executed'] == 0
write('startup.json', data)
def mapping_consumer(src, name, count):
  expected = subprocess.run([sys.executable, '-E', '-B', src], capture_output=True, check=True)
  lines = len(expected.stdout.splitlines())
  assert count is None or lines == count
  with tempfile.TemporaryDirectory() as work:
    binary = Path(work) / 'case'
    rows = {}
    for level in (0, 3):
        for reachable in (False, True):
            flags = ['--reachable-only'] if reachable else []
            subprocess.run([imre, 'build', src, '--runtime', runtime, '--clang', clang,
                            '--opt-level', str(level), '--no-cache', '-o', binary, *flags], check=True)
            actual = subprocess.run([binary], cwd=work, capture_output=True, check=True)
            assert (actual.stdout, actual.stderr) == (expected.stdout, expected.stderr)
            rows[f'O{level}-reachable-{int(reachable)}'] = {'lines': lines, 'stdout_sha256': hashlib.sha256(actual.stdout).hexdigest()}
    write(name, rows)

for src, name, count in (
        (root/'tests/fixtures/classes/mapping_abc.py','mapping-four-modes.json',33),
        (root/'tests/fixtures/classes/mapping_alias.py','mapping-alias-four-modes.json',14),
        (suite.parent/'string/template_differential.py','template-four-modes.json',None),
        (suite.parent/'unittest/mock_reset.py','mock-four-modes.json',None),
        (root/'tests/fixtures/classes/initializer_hook.py','initializer-four-modes.json',None)):
    mapping_consumer(src, name, count)

for level in (0, 3):
    common = ['--imre', imre, '--runtime', runtime, '--clang', clang, '--compiler-revision', revision,
              '--opt-level', str(level)]
    for runner in ('routines', 'doctests', 'counter_host'):
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
    focus = {rel: fixture(rel, level) for rel in ('classes/mapping_abc.py', 'text/line_methods.py', 'containers/namedtuple.py', 'module_values/namedtuple/main.py', 'bytes/membership.py', 'text/percent_scalar.py', 'processes/file_redirects.py', 'json_records/stdlib_deep_decode.py', 'classes/constant_strings.py', 'classes/property_setters.py', 'text/expandtabs.py', 'text/translate.py', 'callables/forward_refs.py')}
    write(f'fixtures-O{level}.json', focus)
subprocess.run([sys.executable, suite / 'native_report.py',
                '--routines', out / 'routines-O0.json', out / 'routines-O3.json',
                '--doctests', out / 'doctests-O0.json', out / 'doctests-O3.json',
                '--host', out / 'counter_host-O0.json', out / 'counter_host-O3.json',
                '--result', out / 'evidence-linux.json'], check=True)
subprocess.run([sys.executable, suite.parents[1] / 'suite_evidence.py', suite / 'reference-linux.json',
                out / 'evidence-linux.json'], check=True)

subprocess.run([sys.executable, root / 'benchmarks/cpython_stdlib/full/types/run.py',
                '--imre', imre, '--runtime', runtime, '--clang', clang, '--cpython-source', root / '.cpython-collections',
                '--result', out / 'types-integration.json'], check=True)
with (out / 'file-ownership.txt').open('w') as log:
    subprocess.run([sys.executable, root / 'tests/files.py', '--compiler', imre, '--runtime', runtime,
                    '--clang', clang, '--opt-levels', '0', '3'], stdout=log, stderr=subprocess.STDOUT, check=True)
