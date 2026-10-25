"""Run the unchanged Path.samefile bodies in their original concrete filesystem fixtures."""

from collections import Counter
import hashlib
import os
import posixpath
import re
import sys

from native_traversal import Bridge, MODULE
from suite_evidence import roles
from suite_native import NativeCompileError, method_bytes, native_method


SAMEFILE_CALLS = {'new': 4, 'same_path': 5, 'same_text': 5, 'join2': 2, 'join3': 1}
SAMEFILE_ASSERTIONS = {'assertTrue': 2, 'assertFalse': 2, 'assertRaises': 6}

DRIVER = '''from pathlib import Path
import posixpath
import sys

def arg(index: int) -> str:
    with open(sys.argv[1] + '/' + str(index), 'rb') as stream:
        return stream.read().decode()

paths: list[Path] = []
print('READY')
while True:
    op = input()
    if op == 'quit':
        break
    if op == 'new':
        paths.append(Path(arg(0)))
        print(len(paths) - 1)
    elif op == 'join2':
        print(repr(posixpath.join(arg(0), arg(1))))
    elif op == 'join3':
        print(repr(posixpath.join(arg(0), arg(1), arg(2))))
    else:
        p = paths[int(arg(0))]
        try:
            if op == 'same_path':
                print(p.samefile(paths[int(arg(1))]))
            elif op == 'same_text':
                print(p.samefile(arg(1)))
            else:
                raise AssertionError('unknown samefile operation')
        except FileNotFoundError as error:
            print('!FileNotFoundError ' + repr(str(error)))
'''


def driven_samefile(tools, directory, case, original, reason):
    ctor = case.cls.__name__
    program = DRIVER.replace('from pathlib import Path', f'from pathlib import {ctor} as Path')
    bridge = Bridge(tools.build(program.encode(), directory), directory, os.getcwd())

    class NativeParser:
        def join(self, *parts):
            if len(parts) not in (2, 3) or any(type(p) is not str for p in parts):
                raise AssertionError('unexpected original parser.join arguments')
            return bridge.call('join' + str(len(parts)), *parts)

    class NativePath:
        def __init__(self, spelling):
            self.handle = bridge.call('new', spelling)

        def samefile(self, other):
            if isinstance(other, NativePath):
                return bridge.call('same_path', self.handle, other.handle)
            if type(other) is not str:
                raise AssertionError(f'unexpected upstream samefile operand: {type(other)}')
            return bridge.call('same_text', self.handle, other)

    assertions, forbidden = Counter(), set()
    original_code = original.__func__.__code__

    def audit(frame, event, arg):
        if event == 'call':
            module, name = frame.f_globals.get('__name__', ''), frame.f_code.co_name
            if module == 'unittest.case' and name in ('assertTrue', 'assertFalse', 'assertRaises'):
                assertions[name] += 1
            if (module.split('.')[0] == 'pathlib'
                    or (module in ('posixpath', 'genericpath') and name in ('samefile', 'samestat'))
                    or (module == 'posixpath' and name == 'join'
                        and frame.f_back and frame.f_back.f_code is original_code)):
                forbidden.add(frame.f_code.co_qualname)
        elif event == 'c_call' and arg in (os.stat, os.lstat):
            forbidden.add(arg.__module__ + '.' + arg.__name__)

    previous = sys.getprofile()
    if 'cls' in vars(case) or vars(case).get('parser') is not posixpath:
        raise ValueError('unexpected original path factory or parser')
    parser = case.parser
    case.cls = NativePath
    case.parser = NativeParser()
    try:
        sys.setprofile(audit)
        original()
    finally:
        sys.setprofile(previous)
        del case.cls
        case.parser = parser
        bridge.close()
    if forbidden or bridge.calls != SAMEFILE_CALLS or assertions != SAMEFILE_ASSERTIONS:
        raise AssertionError((forbidden, bridge.calls, assertions))
    return {'roles': roles(body='cpython', assertions='cpython', operation='imre', fixture='cpython'),
            'evidence': {'operation': 'three native parser joins, four native constructors and ten native samefile calls',
                         'calls': dict(bridge.calls), 'host_assertions': dict(assertions),
                         'fixture': 'original CPython filesystem setup and teardown; child holds native path objects',
                         'whole_body_blocker': reason, 'opt_level': tools.level,
                         'method_sha256': hashlib.sha256(method_bytes(MODULE, 'PathTest', 'test_samefile')).hexdigest()}}


def samefile_adapters(tools, work):
    def run(case, original):
        ctor = case.cls.__name__
        if (case.parser is not posixpath or case.cls.__module__ != 'pathlib'
                or type(case).__name__ != ctor + 'Test'):
            raise ValueError(f'unexpected samefile fixture: {case.id()}')
        directory = work / case.id()
        directory.mkdir()
        fixture = f'''import posixpath
import unittest
from pathlib import {ctor}
class NativeParser:
    def join(self, root: str, first: str, extra: str | None = None) -> str:
        if extra is None:
            return posixpath.join(root, first)
        return posixpath.join(root, first, extra)
class NativeCase(unittest.TestCase):
    base: str
    parser: NativeParser
    def __init__(self) -> None:
        super().__init__()
        self.base = {case.base!r}
        self.parser = NativeParser()
    def cls(self, spelling: str) -> {ctor}:
        return {ctor}(spelling)
'''
        try:
            return native_method(tools, directory, MODULE, 'PathTest', case.id(), fixture,
                                 operation=('imre', 'Path.samefile for text and path operands'))(case, original)
        except NativeCompileError as exc:
            match = re.search(r'case\.py:(\d+):\d+: Imre:', str(exc))
            if not match or 'self.assertRaises(FileNotFoundError,' not in (directory / 'case.py').read_text(
                    encoding='utf-8').splitlines()[int(match[1]) - 1]:
                raise
            reason = str(exc).replace(str(directory), '<case>').strip()
        return driven_samefile(tools, directory, case, original, reason)

    return {f'test.test_pathlib.test_pathlib.{ctor}Test.test_samefile': run for ctor in ('Path', 'PosixPath')}
