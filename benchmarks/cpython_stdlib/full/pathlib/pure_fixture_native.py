"""Find the native boundary for the unchanged pathlib pure fixtures without awarding case credit."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from pure_fixture_probe import BENCH, FAMILIES, SOURCE
from suite_native import method_bytes
from upstream import verify_upstream


def sha(data):
    return hashlib.sha256(data).hexdigest()


def run_case(tool, runtime, clang, entry, root, work, level, reachable=False):
    output = work / 'case'
    command = [tool, 'build', entry, '--module-root', root, '--runtime', runtime, '--clang', clang,
               '--no-cache', '--quiet', '--opt-level', str(level), '-o', output]
    if reachable:
        command.append('--reachable-only')
    built = subprocess.run(command, cwd=work, capture_output=True, timeout=180)
    record = {'level': level, 'reachable_only': reachable, 'build_exit': built.returncode,
              'diagnostic': (built.stdout + built.stderr).decode().replace(str(root) + '/', '<source>/').strip()}
    if not built.returncode:
        executed = subprocess.run([output, '-v'], cwd=work, capture_output=True, timeout=180)
        record.update(exit=executed.returncode, stdout=executed.stdout.decode(), stderr=executed.stderr.decode())
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('artifacts', 'clang', 'cpython-git', 'result'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != 'cpython' or sys.version_info[:3] != (3, 14, 5):
        parser.error('use the test-enabled pinned CPython 3.14.5')
    pin = verify_upstream()
    source = BENCH / 'upstream' / SOURCE
    setup = method_bytes(source, 'PurePathTest', 'setUp')
    real_body = method_bytes(source, 'PurePathTest', 'test_str_common')
    # Slice diagnostics can isolate a compiler feature. Only full-package execution can register original cases.
    header = b'import unittest\nimport pathlib\nimport posixpath\nclass PurePathTest(unittest.TestCase):\n'
    main_call = b'\nunittest.main()\n'
    skip_control = ('import unittest\nclass Gate(unittest.TestCase):\n'
                    '    def setUp(self):\n        self.skipTest({reason!r})\n'
                    '    def test_registered(self):\n        raise AssertionError("the skipped body ran")\n'
                    'unittest.main()\n')
    controls = {
        'original_setup_first_expression': header + setup + real_body + main_call,
        'original_class_factory_and_setup': header + b'    cls = pathlib.PurePath\n' + setup + real_body + main_call,
        'registered_id': b'import unittest\nclass Identity(unittest.TestCase):\n'
                         b'    def setUp(self):\n        print(self.id())\n'
                         b'    def test_registered(self):\n        pass\nunittest.main()\n',
        'class_parser_identity': b'from pathlib import PureWindowsPath\nimport posixpath\n'
                                 b'print(PureWindowsPath.parser is posixpath)\n',
        'instance_parser': b'from pathlib import PureWindowsPath\np = PureWindowsPath("a")\nprint(p.parser.sep)\n',
    }
    for flavour in ('POSIX', 'Windows'):
        controls['skip_' + flavour] = skip_control.format(reason=f'requires {flavour}-flavoured path class').encode()
    driver = ('from test.test_pathlib import test_pathlib as target\nimport unittest\nsuite = unittest.TestSuite()\n'
              + ''.join(f'suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(target.{family}))\n'
                        for family in FAMILIES) + 'unittest.TextTestRunner(verbosity=2).run(suite)\n').encode()
    package = (b'import test.test_pathlib as target\nimport unittest\n'
               b'suite = unittest.defaultTestLoader.loadTestsFromModule(target)\n'
               b'unittest.TextTestRunner(verbosity=2).run(suite)\n')
    meta = json.loads((args.artifacts / 'source.json').read_bytes())
    for name in ('imre', 'libimre_runtime.a'):
        with (args.artifacts / name).open('rb') as stream:
            if hashlib.file_digest(stream, 'sha256').hexdigest() != meta['files'][name]:
                raise ValueError(f'artifact hash differs: {name}')
    tool, runtime = [(args.artifacts / name).resolve() for name in ('imre', 'libimre_runtime.a')]
    record = {'schema': 'imre.pathlib.pure-fixture-boundaries/v1', 'cpython_commit': pin, 'source': meta,
              'setup_sha256': sha(setup), 'control_method': 'PurePathTest.test_str_common',
              'control_method_sha256': sha(real_body), 'attempts': {}}
    with tempfile.TemporaryDirectory(prefix='pathlib-pure-') as temporary:
        work = Path(temporary).resolve()
        # Stage the entire pinned test package and all its original test.support dependencies in one archive.
        archive = work / 'tests.tar'
        subprocess.run(['git', '-C', args.cpython_git, 'archive', '--format=tar', '-o', archive,
                        pin, '--', 'Lib/test'], check=True)
        with tarfile.open(archive) as tar:
            tar.extractall(work, filter='data')
        root = work / 'Lib'
        for name, content in {'original_package': package, 'original_pure_classes': driver, **controls}.items():
            original = name in ('original_package', 'original_pure_classes')
            with tempfile.TemporaryDirectory(dir=work) as case_dir:
                case_dir = Path(case_dir)
                module_root = root if original else case_dir
                entry = module_root / 'probe.py'
                entry.write_bytes(content)
                attempts = [run_case(tool, runtime, args.clang.resolve(), entry, module_root, case_dir, level, reach)
                            for reach in ((False, True) if original else (False,)) for level in (0, 3)]
                if original and any(row['build_exit'] == 0 for row in attempts):
                    raise ValueError(f'{name} now builds; verify every registered ID and role before promotion')
                if name.startswith('skip_'):
                    reason = f'requires {name.removeprefix("skip_")}-flavoured path class'
                    if any(row.get('exit') != 0 or 'Ran 1 test' not in row['stderr'] or
                           f"skipped '{reason}'" not in row['stderr'] or 'OK (skipped=1)' not in row['stderr']
                           or row['stdout'] for row in attempts):
                        raise ValueError(f'{name}: native skip semantics differ: {attempts}')
                record['attempts'][name] = {'scope': 'original registration entry' if original else 'diagnostic only',
                                            'source_sha256': sha(content), 'results': attempts}
                print(name, [(r['level'], r['reachable_only'], r['build_exit']) for r in attempts], flush=True)
    record['fully_native_original_cases_observed'] = 0
    args.result.write_text(json.dumps(record, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
