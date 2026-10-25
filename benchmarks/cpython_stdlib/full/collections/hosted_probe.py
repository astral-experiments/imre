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
for level in (0, 3):
    for name, size in [('TestCounter', 22), ('TestChainMap', 10)]:
        path = out / f'{name}-O{level}.json'
        subprocess.run([sys.executable, suite / 'routines.py', '--imre', imre, '--runtime', runtime, '--clang', clang,
                        '--compiler-revision', revision, '--opt-level', str(level), '--class', name, '--result', path],
                       check=True)
        rows = load(path)['routines']
        assert len(rows) == size and not {k: v for k, v in rows.items() if v['result'] in ('failed', 'fixture-skip')}
    compare = {p: fixture(p, level) for p in ('containers/dict_copy_update.py', 'containers/counter_subtract.py',
                'containers/counter_arithmetic.py', 'containers/dict_clear.py', 'containers/array_registration.py',
                'classes/property_setters.py', 'callables/cache_parameters.py')}
    assert compare['containers/dict_copy_update.py']['lines'] == 29
    assert compare['containers/array_registration.py']['lines'] == 532
    write(f'fixtures-O{level}.json', compare)
