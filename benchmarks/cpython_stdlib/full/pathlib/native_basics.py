"""Run the string roundtrips, absolute POSIX paths and root directory methods in Imre."""
import ast
import hashlib
import inspect
import pathlib
import posixpath
import subprocess

from body_sweep import declaration, raw
from suite_evidence import roles
from suite_native import method_bytes


# Number of native constructor calls and assertions in each complete method.
CHECKS = {
    'test_empty_path': (2, 2, 'empty-path construction and str'),
    'test_str_common': (6, 6, 'str of canonical paths'),
    'test_absolute_posix': (6, 6, 'Path.absolute, including double-slash roots'),
    'test_mkdir_exist_ok_root': (2, 0, 'Path.resolve and Path.mkdir on an existing root'),
}


def basic_adapters(tools, directory):
    def run(case, original):
        name = case._testMethodName
        ctor = case.cls.__name__
        if case.cls is not getattr(pathlib, ctor) or case.sep != ('\\' if ctor == 'PureWindowsPath' else '/'):
            raise ValueError(f'wrong original class or separator for {case.id()}')
        owner, path, _, lines, node = declaration(type(case), name)
        if original.__func__ is not vars(owner)[name]:
            raise ValueError(f'unexpected registered function for {case.id()}')
        decorators = [ast.unparse(d) for d in node.decorator_list]
        if name == 'test_absolute_posix':
            # This identity decorator registers the method. The original setUp checks its name.
            if (decorators != ['needs_posix'] or case.cls.parser is not posixpath
                    or name not in inspect.getmodule(original)._tests_needing_posix):
                raise ValueError(f'original POSIX gate did not select {case.id()}')
        elif decorators:
            raise ValueError(f'unexpected wrapper for {case.id()}')

        body = raw(lines, node)
        helper = (method_bytes(path, 'PurePathTest', '_check_str')
                  if name in ('test_empty_path', 'test_str_common') else b'')
        fixture = f'''from pathlib import {ctor}
class NativeCase:
    sep: str
    calls: int
    checks: int
    def __init__(self) -> None:
        self.sep = {case.sep!r}
        self.calls = 0
        self.checks = 0
    def cls(self, value: str) -> {ctor}:
        self.calls += 1
        return {ctor}(value)
    def assertEqual(self, left: str, right: str) -> None:
        if left != right:
            raise AssertionError('different path strings')
        self.checks += 1
'''
        entry = fixture.encode() + helper + body + f'\nc = NativeCase()\nc.{name}()\nprint(c.calls)\nprint(c.checks)\n'.encode()
        executable = tools.build(entry, directory)
        result = subprocess.run([executable], capture_output=True, text=True, encoding='utf8', timeout=60)
        calls, assertions, operation = CHECKS[name]
        if result.returncode or result.stderr or result.stdout != f'{calls}\n{assertions}\n':
            raise AssertionError((case.id(), result.returncode, result.stdout, result.stderr))
        evidence = {'operation': operation, 'fixture': 'original CPython setup and class; native receiver',
                    'method_sha256': hashlib.sha256(body).hexdigest(), 'native_constructors': calls,
                    'native_assertions': assertions, 'opt_level': tools.level}
        if helper:
            evidence['helper_sha256'] = hashlib.sha256(helper).hexdigest()
        return {'roles': roles(body='imre', assertions='imre' if assertions else 'none',
                               operation='imre', fixture='mixed'), 'evidence': evidence}

    return {f'test.test_pathlib.test_pathlib.{cls}Test.{name}': run
            for cls in ('Path', 'PosixPath', 'PurePath', 'PurePosixPath', 'PureWindowsPath')
            for name in CHECKS if cls in ('Path', 'PosixPath') or name in ('test_empty_path', 'test_str_common')}
