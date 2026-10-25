"""Exercise lexical component operations under the pinned concrete and pure path fixtures."""
import hashlib
import inspect
import os
from pathlib import Path
import sys

from native_traversal import Bridge
from suite_evidence import roles
from suite_native import native_method

HERE = Path(__file__).resolve().parent
PACKAGE = HERE.parents[1] / 'upstream/Lib/test/test_pathlib'
TYPES = ('Path', 'PosixPath', 'PurePath', 'PurePosixPath', 'PureWindowsPath')
MISSING = object()


def facade(bridge):
    class NativePath:
        def __init__(self, *parts):
            if len(parts) > 1 or any(not isinstance(p, str) for p in parts):
                raise ValueError('unexpected constructor shape in component fixture')
            self.handle = bridge.call('new', len(parts), *parts)

        @classmethod
        def wrap(cls, handle):
            value = object.__new__(cls)
            value.handle = handle
            return value

        def __str__(self):
            return bridge.call('text', self.handle)

        def __eq__(self, other):
            if not isinstance(other, NativePath):
                return NotImplemented
            return bridge.call('equal', self.handle, other.handle)

        def with_name(self, value):
            return self.replace('with_name', value)

        def with_stem(self, value):
            return self.replace('with_stem', value)

        def with_suffix(self, value):
            return self.replace('with_suffix', value)

        def replace(self, command, value):
            if value is not None and not isinstance(value, str):
                raise ValueError('unrecognized replacement operand in fixture')
            return self.wrap(bridge.call(command, self.handle, value or '', 'none' if value is None else 'str'))

        def relative_to(self, other=MISSING, *, walk_up=False):
            return self.wrap(bridge.call('relative_to', self.handle, *operand(other), walk_up))

        def is_relative_to(self, other=MISSING):
            return bridge.call('is_relative_to', self.handle, *operand(other))

    def operand(other):
        if other is MISSING:
            return 'missing', ''
        if isinstance(other, NativePath):
            return 'path', other.handle
        if isinstance(other, (bytes, str)):
            return type(other).__name__, other
        raise ValueError('unrecognized relative path operand in fixture')

    return NativePath


def host_components(binary, directory, case, original, ctor):
    if case.cls.__name__ != ctor or vars(case).get('cls') is not None:
        raise ValueError(f'unexpected original constructor: {case.id()}')
    bridge = Bridge(binary, directory, os.getcwd())
    host = set()

    def profile(frame, event, arg):
        module = frame.f_globals.get('__name__', '')
        if event == 'call' and module.split('.')[0] == 'pathlib':
            host.add(module + '.' + frame.f_code.co_qualname)

    previous = sys.getprofile()
    case.cls = facade(bridge)
    try:
        sys.setprofile(profile)
        original()
    finally:
        sys.setprofile(previous)
        del case.cls
        bridge.close()
    operations = sorted(bridge.calls.keys() - {'new', 'text', 'equal'})
    if not operations:
        raise AssertionError('the upstream method did not reach a native component operation')
    return {'roles': roles(body='cpython', assertions='cpython', operation='mixed' if host else 'imre', fixture='cpython'),
            'evidence': {'operation': ', '.join(operations), 'fixture': 'original upstream class and setup; native path handles',
                         'native_calls': dict(sorted(bridge.calls.items())), 'host_product_calls': sorted(host),
                         'method_source_sha256': hashlib.sha256(inspect.getsource(original).encode()).hexdigest()}}


def component_adapters(tools, work):
    template = (HERE / 'component_driver.py').read_bytes()
    result = {}
    for ctor in TYPES:
        directory = work / ctor
        directory.mkdir()
        binary = tools.build(template.replace(b'PATH_TYPE', ctor.encode()), directory)

        def host(case, original, binary=binary, ctor=ctor):
            scratch = work / case.id()
            scratch.mkdir()
            return host_components(binary, scratch, case, original, ctor)

        def add(module, fixture, method, execute=host):
            result[f'test.test_pathlib.{module}.{fixture}.test_{method}'] = execute

        for method in ('with_name_empty', 'with_stem_empty', 'relative_to_common', 'is_relative_to_common'):
            add('test_pathlib', ctor + 'Test', method)
        if ctor == 'PureWindowsPath':
            for method in ('with_name_windows', 'with_stem_windows', 'relative_to_windows', 'is_relative_to_windows'):
                add('test_pathlib', ctor + 'Test', method)
        if ctor not in ('Path', 'PurePath', 'PureWindowsPath'):
            continue
        module = 'test_join_windows' if ctor == 'PureWindowsPath' else 'test_join'
        fixture = ctor + 'JoinTest'
        for method in ('with_name', 'with_stem', 'with_suffix'):
            add(module, fixture, method)
        case_id = f'test.test_pathlib.{module}.{fixture}.test_suffixes'
        typed = f'''from pathlib import {ctor}
class NativeCase:
    def cls(self, value: str) -> {ctor}:
        return {ctor}(value)
    def assertEqual(self, left: list[str], right: list[str]) -> None:
        if len(left) != len(right):
            raise AssertionError('different suffixes')
        for i in range(len(left)):
            if left[i] != right[i]:
                raise AssertionError('different suffixes')
'''
        subdir = directory / 'suffixes'
        subdir.mkdir()
        result[case_id] = native_method(tools, subdir, PACKAGE / (module + '.py'), 'JoinTestBase',
                                        case_id, typed, operation=('imre', 'PurePath.suffixes'))
    return result
