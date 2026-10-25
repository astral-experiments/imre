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
from suite_evidence import roles
from suite_inventory import load_suite, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, native_method, run_in_fixtures
from upstream import verify_upstream

MODULE = BENCHMARK / 'upstream/Lib/test/test_pathlib/test_pathlib.py'
PREFIX = 'test.test_pathlib.test_pathlib.'
READ = 'test.test_pathlib.test_read.PathReadTest.'


class Bridge:
    """Only the native process holds Path values, path-key tables, and cursor positions."""
    def __init__(self, binary, directory, cwd):
        self.directory = os.fspath(directory)
        self.cwd = cwd
        self.calls = Counter()
        self.host = set()
        self.process = subprocess.Popen([str(binary), str(directory)], cwd=cwd, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if self.read_line() != 'READY\n':
            raise RuntimeError(self.process.stderr.read())

    def read_line(self):
        if not select.select([self.process.stdout], [], [], 20)[0]:
            self.process.kill()
            raise TimeoutError('native traversal driver did not answer')
        return self.process.stdout.readline()

    def call(self, name, *args):
        if name in ('glob', 'rglob', 'iterdir', 'next', 'exists', 'mkdir') and os.getcwd() != self.cwd:
            raise RuntimeError('native fixture needs to follow the upstream cwd change')
        for i, value in enumerate(args):
            with open(os.path.join(self.directory, str(i)), 'wb') as out:
                out.write(value if isinstance(value, bytes) else str(value).encode())
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
            raise {'OSError': OSError, 'FileNotFoundError': FileNotFoundError,
                   'NotADirectoryError': NotADirectoryError, 'PermissionError': PermissionError,
                   'TypeError': TypeError, 'ValueError': ValueError,
                   'NotImplementedError': NotImplementedError}[kind](ast.literal_eval(message))
        return ast.literal_eval(output)

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.write('quit\n')
            self.process.stdin.close()
            self.process.wait(timeout=10)
        extra = self.process.stdout.read() + self.process.stderr.read()
        self.process.stdout.close()
        self.process.stderr.close()
        if self.process.returncode or extra:
            raise RuntimeError(f'native cleanup: {self.process.returncode}: {extra}')


def facade(bridge):
    class Parts:
        def __init__(self, number):
            self.number = number

        def count(self, value):
            return bridge.call('count_part', self.number, value)

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
        def parts(self):
            return Parts(self.number)

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
    if case.id() == READ + 'test_iterdir':
        fixture = f"""from pathlib import Path
class Ground:
    can_symlink: bool
    def __init__(self) -> None:
        self.can_symlink = {case.ground.can_symlink!r}
class NativeCase:
    ground: Ground
    root: Path
    def __init__(self) -> None:
        self.ground = Ground()
        self.root = Path({os.fspath(case.root)!r})
    def assertEqual(self, first: set[Path], second: set[Path]) -> None:
        if first != second:
            raise AssertionError("different directory members")
"""
        return native_method(tools, directory, MODULE.with_name('test_read.py'),
                             'ReadTestBase', case.id(), fixture, operation=('imre', 'Path.iterdir'))(case, original)
    # These IDs have the same definition but distinct concrete fixture classes.
    ctor = case.cls.__name__
    if case._testMethodName == 'test_glob_case_sensitive':
        source = f'''from pathlib import {ctor}
class NativeCase:
    base: str
    def __init__(self) -> None:
        self.base = {case.base!r}
    def cls(self, value: str, child: str | None = None) -> {ctor}:
        if child is None:
            return {ctor}(value)
        return {ctor}(value, child)
    def assertEqual(self, left: set[str], right: set[str]) -> None:
        if left != right:
            raise AssertionError('different path spellings')
'''
        return native_method(tools, directory, MODULE, 'PathTest', case.id(), source,
                             operation=('imre', 'Path.glob with an explicit case_sensitive option'))(case, original)
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
    return native_method(tools, directory, MODULE, 'PathTest', case.id(), source,
                         operation=('imre', 'Path.glob and Path.rglob across suspended cursors'))(case, original)


def registered_body(case, original, owner_name, gate):
    """The original fixture checks this identity decorator's registered gate in setUp."""
    name = case._testMethodName
    module = inspect.getmodule(original)
    if (original.__func__ is not vars(getattr(module, owner_name))[name]
            or name not in getattr(module, '_tests_needing_' + gate)
            or not (case.cls.parser is posixpath if gate == 'posix' else case.can_symlink)):
        raise ValueError(f'original {gate} fixture did not select {case.id()}')
    data = MODULE.read_bytes()
    owner, = [c for c in ast.parse(data).body if isinstance(c, ast.ClassDef) and c.name == owner_name]
    method, = [m for m in owner.body if isinstance(m, ast.FunctionDef) and m.name == name]
    if [ast.unparse(d) for d in method.decorator_list] != ['needs_' + gate]:
        raise ValueError(f'unexpected decorator on {name}')
    return b''.join(data.splitlines(keepends=True)[method.lineno - 1:method.end_lineno])


def native_registered_rglob(tools, directory, case, original):
    name, ctor = case._testMethodName, case.cls.__name__
    posix = name == 'test_rglob_posix'
    gate = 'posix' if posix else 'symlinks'
    body = registered_body(case, original, 'PathTest', gate)
    parameters, arguments = ('value: str, child: str', 'value, child') if posix else ('value: str', 'value')
    fixture = f'''from pathlib import {ctor}
class NativeCase:
    base: str
    checks: int
    def __init__(self) -> None:
        self.base = {case.base!r}
        self.checks = 0
    def cls(self, {parameters}) -> {ctor}:
        return {ctor}({arguments})
    def assertEqual(self, left: set[{ctor}], right: set[{ctor}]) -> None:
        if left != right:
            raise AssertionError('different recursive glob paths')
        self.checks += 1
'''
    entry = fixture.encode() + body + f'\ncase = NativeCase()\ncase.{name}()\nprint(case.checks)\n'.encode()
    binary = tools.build(entry, directory)
    result = subprocess.run([binary], capture_output=True, timeout=60, check=True)
    assertions = 2 if posix else 1
    if result.stderr or result.stdout != f'{assertions}\n'.encode():
        raise AssertionError((case.id(), result.stdout, result.stderr))
    detail = {'operation': 'Path.rglob and native set[Path] comparisons',
              'fixture': f'upstream needs_{gate} registry and setUp; native test instance',
              'opt_level': tools.level, 'assertions': assertions, 'method_sha256': hashlib.sha256(body).hexdigest()}
    return {'execution': 'native-body', 'fixture_execution': detail['fixture'], 'method_sha256': detail['method_sha256'],
            'observation': {'roles': roles(body='imre', assertions='imre', operation='imre', fixture='mixed'),
                            'evidence': detail}}


def host_method(binary, directory, case, original):
    # change_cwd runs in the original test. Start its independent native process
    # in that same directory; all filesystem requests check the two cwd values.
    cwd = os.path.join(case.base, 'dirC') if case._testMethodName == 'test_glob_dot' else os.getcwd()
    bridge = Bridge(binary, directory, cwd)
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
    calls = dict(sorted(bridge.calls.items()))
    host = sorted(host_calls | bridge.host)
    proof = {'operation': ', '.join(k for k in ('glob', 'rglob', 'iterdir') if k in calls),
             'fixture': 'original CPython hooks; path and cursor values held by Imre',
             'native_calls': calls, 'host_product_calls': host,
             'method_source_sha256': hashlib.sha256(body).hexdigest()}
    observation = {'roles': roles(body='cpython', assertions='cpython',
                                  operation='mixed' if host else 'imre', fixture='cpython'),
                   'evidence': proof}
    return {'execution': 'mixed' if host else 'host-driven-native-operations',
            'body': 'CPython', 'assertions': 'CPython', 'fixture_execution': 'original CPython hooks',
            'native_calls': calls, 'host_product_calls': host,
            'method_source_sha256': proof['method_source_sha256'], 'observation': observation}


HOST = ('test_glob_empty_pattern', 'test_glob_case_sensitive', 'test_glob_recurse_symlinks_common',
        'test_rglob_recurse_symlinks_common', 'test_rglob_recurse_symlinks_false', 'test_rglob_symlink_loop',
        'test_glob_pathlike', 'test_rglob_pathlike', 'test_glob_dotdot', 'test_glob_dot',
        'test_glob_above_recursion_limit', 'test_rglob_posix', 'test_iterdir_symlink')


def traversal_adapters(tools, work):
    binary = tools.build((HERE / 'traversal_driver.py').read_bytes(), work)
    def run(case, original):
        directory = work / case.id()
        directory.mkdir()
        if case.id().endswith('test_glob_many_open_files'):
            return native_body(tools, directory, case, original)
        if case._testMethodName in ('test_rglob_posix', 'test_rglob_symlink_loop'):
            return native_registered_rglob(tools, directory, case, original)
        fallback = {
            'test_iterdir': ('arithmetic requires int or float operands', 'expected += [...]'),
            'test_glob_case_sensitive': ('local functions require annotated parameters and return types',
                                         'nested _check(path, pattern, case_sensitive, expected)'),
        }.get(case._testMethodName)
        if fallback:
            try:
                return native_body(tools, directory, case, original)
            except NativeCompileError as exc:
                diagnostic = str(exc).strip().split('Imre: ')[-1]
                # Run the entire routine under its original fixtures and keep
                # this failed native-body attempt separate from operations.
                if diagnostic != fallback[0]:
                    raise
                evidence = host_method(binary, directory, case, original)
                evidence['native_body_blocker'] = {'operation': fallback[1], 'diagnostic': diagnostic}
                evidence['observation']['evidence']['native_body_blocker'] = evidence['native_body_blocker']
                return evidence
        return host_method(binary, directory, case, original)
    ids = {PREFIX + cls + '.' + name:run for cls in ('PathTest','PosixPathTest')
           for name in (*HOST, 'test_glob_many_open_files')}
    ids.update({READ + name: run for name in ('test_glob', 'test_iterdir', 'test_iterdir_nodir')})
    return ids


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
        ids = traversal_adapters(tools, work)
        evidence, failures, skips = run_in_fixtures(load_suite('test.test_pathlib'), ids)
    report = {'cpython_commit': commit, 'opt_level': args.opt_level, 'evidence': evidence,
              'failures': failures, 'skips': skips}
    args.result.write_text(json.dumps(report, indent=2) + '\n')
    if failures or skips or set(ids) != set(evidence) or any(len(v) != 1 for v in evidence.values()):
        raise RuntimeError([(i,m.strip().splitlines()[-1]) for i,m in failures] + skips)
    print(dict(Counter(v[0]['execution'] for v in evidence.values())))


if __name__ == '__main__':
    main()
