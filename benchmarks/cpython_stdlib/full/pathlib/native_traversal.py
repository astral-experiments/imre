"""Run complete original glob methods with native path values and suspended native cursors."""
import argparse
import ast
from collections import Counter
from collections.abc import Iterator
from contextlib import chdir
import hashlib
import inspect
import json
import os
from pathlib import Path
import posixpath
import select
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BENCHMARK = HERE.parents[1]
sys.path.insert(0, str(BENCHMARK))
from suite_inventory import load_suite, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures
from upstream import verify_upstream

MODULE = BENCHMARK / 'upstream/Lib/test/test_pathlib/test_pathlib.py'
PREFIX = 'test.test_pathlib.test_pathlib.'
READ = 'test.test_pathlib.test_read.PathReadTest.test_glob'


class Bridge:
    """Only the native process holds Path values, path-key tables, and cursor positions."""
    def __init__(self, binary, directory):
        self.directory = os.fspath(directory)
        self.cwd = os.getcwd()
        self.calls = Counter()
        self.host = set()
        self.process = subprocess.Popen([str(binary), str(directory)], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if self.read_line() != 'READY\n':
            raise RuntimeError(self.process.stderr.read())

    def read_line(self):
        if not select.select([self.process.stdout], [], [], 20)[0]:
            self.process.kill()
            raise TimeoutError('native traversal driver did not answer')
        return self.process.stdout.readline()

    def call(self, name, *args):
        if os.getcwd() != self.cwd:
            raise RuntimeError('native fixture needs to follow the upstream cwd change')
        for i, value in enumerate(args):
            with open(os.path.join(self.directory, str(i)), 'wb') as out:
                out.write(str(value).encode())
        self.process.stdin.write(name + '\n')
        self.process.stdin.flush()
        output = self.read_line()
        if not output:
            raise RuntimeError(f'native process exited during {name}: {self.process.stderr.read()}')
        self.calls[name] += 1
        if output.startswith('!StopIteration'):
            raise StopIteration
        if output.startswith('!'):
            kind, message = output[1:].split(' ', 1)
            raise {'OSError': OSError, 'ValueError': ValueError,
                   'NotImplementedError': NotImplementedError}[kind](ast.literal_eval(message))
        return ast.literal_eval(output)

    def close(self):
        self.process.stdin.write('quit\n')
        self.process.stdin.close()
        self.process.wait(timeout=10)
        extra = self.process.stdout.read() + self.process.stderr.read()
        self.process.stdout.close()
        self.process.stderr.close()
        if self.process.returncode or extra:
            raise RuntimeError(f'native cleanup: {self.process.returncode}: {extra}')


def facade(bridge):
    class Cursor(Iterator):
        def __init__(self, number):
            self.number = number

        def __iter__(self):
            assert bridge.call('iter', self.number) is True
            return self

        def __next__(self):
            return NativePath.wrap(bridge.call('next', self.number))

    class PathClass(type):
        @property
        def parser(cls):
            bridge.host.add('path parser module identity')
            return posixpath

    class NativePath(metaclass=PathClass):
        def __init__(self, *parts):
            self.number = bridge.call('new', len(parts), *(os.fspath(p) for p in parts))

        @classmethod
        def wrap(cls, number):
            p = object.__new__(cls)
            p.number = number
            return p

        def __str__(self):
            return bridge.call('text', self.number)

        __fspath__ = __str__

        def __repr__(self):
            return f'NativePath({str(self)!r})'

        def __hash__(self):
            # The native dict[Path, int] assigns equal native paths the same key.
            # This is a transport key for the host assertion, not a hash() test.
            return self.number

        def __eq__(self, other):
            if not isinstance(other, NativePath):
                return NotImplemented
            return bridge.call('equal', self.number, other.number)

        def joinpath(self, *parts):
            return self.wrap(bridge.call('join', self.number, len(parts), *(os.fspath(p) for p in parts)))

        def __truediv__(self, other):
            return self.joinpath(other)

        @property
        def parent(self):
            return self.wrap(bridge.call('parent', self.number))

        @property
        def parser(self):
            return type(self).parser

        def exists(self):
            return bridge.call('exists', self.number)

        def mkdir(self, mode=0o777, parents=False, exist_ok=False):
            return bridge.call('mkdir', self.number, mode, str(parents).lower(), str(exist_ok).lower())

        def iterdir(self):
            return Cursor(bridge.call('iterdir', self.number))

        def _glob(self, command, pattern, case_sensitive, recurse_symlinks):
            if isinstance(pattern, NativePath):
                value, form = pattern.number, 'path'
            elif isinstance(pattern, str):
                value, form = pattern, 'str'
            else:
                bridge.host.add('third-party __fspath__ conversion')
                value, form = os.fspath(pattern), 'str'
            return Cursor(bridge.call(command, self.number, value, str(case_sensitive).lower(),
                                      str(recurse_symlinks).lower(), form))

        def glob(self, pattern, *, case_sensitive=None, recurse_symlinks=False):
            return self._glob('glob', pattern, case_sensitive, recurse_symlinks)

        def rglob(self, pattern, *, case_sensitive=None, recurse_symlinks=False):
            return self._glob('rglob', pattern, case_sensitive, recurse_symlinks)

    return NativePath


def native_body(tools, directory, case, original):
    # These IDs have the same definition but distinct concrete fixture classes.
    ctor = case.cls.__name__
    source = f"""from pathlib import {ctor}
class NativeCase:
    base: str
    def __init__(self) -> None:
        self.base = {os.fspath(case.base)!r}
    def cls(self, value: str) -> {ctor}:
        return {ctor}(value)
    def assertEqual(self, first: {ctor}, second: {ctor}) -> None:
        if first != second:
            raise AssertionError("different paths")
"""
    return native_method(tools, directory, MODULE, 'PathTest', case.id(), source)(case, original)


def host_method(binary, directory, case, original):
    bridge = Bridge(binary, directory)
    cls = facade(bridge)
    saved = {}
    patches = {'cls': cls} if hasattr(case, 'cls') else {}
    if hasattr(case, 'root'):
        patches['root'] = cls(os.fspath(case.root))
    host_calls = set()

    def profile(frame, event, arg):
        if event == 'call' and frame.f_code.co_qualname == 'TestCase.assertIsInstance':
            obj = frame.f_locals.get('obj')
            if type(obj).__qualname__.endswith('facade.<locals>.Cursor'):
                bridge.host.add('iterator type assertion on host facade')
        if event == 'call' and frame.f_globals.get('__name__', '').split('.')[0] == 'pathlib':
            host_calls.add(frame.f_globals['__name__'] + '.' + frame.f_code.co_qualname)

    # Trace actual host use of pathlib in the test, not the original setup/cleanup.
    # Transfer files use builtins.open so marshaling itself does not call host pathlib.
    old = sys.getprofile()
    try:
        for key, value in patches.items():
            saved[key] = (key in case.__dict__, getattr(case, key))
            setattr(case, key, value)
        sys.setprofile(profile)
        original()
    finally:
        sys.setprofile(old)
        for key, (had, value) in saved.items():
            if had:
                setattr(case, key, value)
            else:
                delattr(case, key)
        bridge.close()
    if not bridge.calls['glob'] and not bridge.calls['rglob'] and not bridge.calls['iterdir']:
        raise AssertionError('no native traversal reached')
    body = inspect.getsource(inspect.unwrap(original)).encode()
    return {'execution': 'mixed' if host_calls or bridge.host else 'host-driven-native-operations',
            'body': 'CPython', 'assertions': 'CPython', 'fixture_execution': 'original CPython hooks',
            'native_calls': dict(sorted(bridge.calls.items())),
            'host_product_calls': sorted(host_calls | bridge.host),
            'method_source_sha256': hashlib.sha256(body).hexdigest()}


HOST = ('test_glob_empty_pattern', 'test_glob_case_sensitive', 'test_glob_recurse_symlinks_common',
        'test_rglob_recurse_symlinks_common', 'test_rglob_recurse_symlinks_false', 'test_rglob_symlink_loop',
        'test_glob_pathlike', 'test_rglob_pathlike', 'test_glob_dotdot', 'test_iterdir_symlink')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('imre', 'runtime', 'clang', 'result'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--opt-level', choices=(0, 3), type=int, required=True)
    parser.add_argument('--cpython-source', type=Path)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or os.name != 'posix':
        parser.error('use the test-enabled CPython 3.14.5 on POSIX')
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / 'Lib'))
    commit = verify_upstream()
    manifest = json.loads((BENCHMARK / 'UPSTREAM.json').read_text())
    verify_interpreter_tests({'files': {k:v for k,v in manifest['files'].items()
                                       if k.startswith('Lib/test/test_pathlib/')}})
    tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), args.opt_level)
    with tempfile.TemporaryDirectory(prefix='pl-') as tmp, chdir(tmp):
        work = Path(tmp)
        binary = tools.build((HERE / 'traversal_driver.py').read_bytes(), work)
        def run(case, original):
            directory = work / case.id()
            directory.mkdir()
            if case.id().endswith('test_glob_many_open_files'):
                return native_body(tools, directory, case, original)
            return host_method(binary, directory, case, original)
        ids = {PREFIX + cls + '.' + name:run for cls in ('PathTest','PosixPathTest')
               for name in (*HOST, 'test_glob_many_open_files')}
        ids[READ] = run
        evidence, failures, skips = run_in_fixtures(load_suite('test.test_pathlib'), ids)
    report = {'cpython_commit': commit, 'opt_level': args.opt_level, 'evidence': evidence,
              'failures': failures, 'skips': skips}
    args.result.write_text(json.dumps(report, indent=2) + '\n')
    if failures or skips or set(ids) != set(evidence) or any(len(v) != 1 for v in evidence.values()):
        raise RuntimeError([(i,m.strip().splitlines()[-1]) for i,m in failures] + skips)
    print(dict(Counter(v[0]['execution'] for v in evidence.values())))


if __name__ == '__main__':
    main()
