"""Replay complete stat/lstat methods under their original Linux fixtures."""
from collections import Counter
import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys

from native_traversal import Bridge, MODULE, registered_body
from suite_evidence import roles
from suite_native import NativeCompileError, method_bytes, native_method

METHODS = ('test_stat', 'test_stat_no_follow_symlinks_nosymlink', 'test_stat_no_follow_symlinks',
           'test_lstat', 'test_lstat_nosymlink')
SYMLINK = {'test_stat_no_follow_symlinks', 'test_lstat'}
FIELD_CALLS = {'new': 3, 'join': 3, 'stat': 3, 'st_mode': 7, 'st_ino': 7, 'st_dev': 5}
FIELD_ASSERTIONS = {'assertIsInstance': 3, 'assertEqual': 3, 'assertNotEqual': 5}


def check_fixture_stats(binary, work, case):
    # This independent probe compares each real fixture path with CPython.
    # The original method below performs its own cross-path assertions.
    bridge = Bridge(binary, work, os.getcwd())
    devices = {}
    try:
        for name in ('fileA', 'dirB/fileB', 'dirC'):
            path = os.path.join(case.base, name)
            host = os.stat(path)
            result = bridge.call('stat', bridge.call('new', path))
            for field in ('st_mode', 'st_ino', 'st_dev'):
                native = bridge.call(field, result)
                if native != getattr(host, field):
                    raise AssertionError((path, field, getattr(host, field), native))
            devices[name] = host.st_dev
    finally:
        bridge.close()
    print(f'{case.id()} CPython/Imre fixture st_dev: {devices}', flush=True)


def driven_stat(tools, work, case, original, failure):
    ctor = case.cls.__name__
    driver = Path(__file__).with_name('stat_driver.py').read_bytes().replace(
        b'from pathlib import Path', f'from pathlib import {ctor} as Path'.encode())
    binary = tools.build(driver, work)
    check_fixture_stats(binary, work, case)
    bridge = Bridge(binary, work, os.getcwd())

    class Result:
        def __init__(self, handle):
            self.handle = handle

        def __getattr__(self, field):
            if field not in ('st_mode', 'st_ino', 'st_dev'):
                raise AttributeError(field)
            return bridge.call(field, self.handle)

    class NativePath:
        def __init__(self, name):
            self.handle = bridge.call('new', name)

        def joinpath(self, *parts):
            if len(parts) not in (1, 2):
                raise ValueError('unexpected stat fixture join arity')
            path = object.__new__(NativePath)
            path.handle = bridge.call('join', self.handle, len(parts), *parts)
            return path

        def stat(self):
            return Result(bridge.call('stat', self.handle))

    assertions = Counter()
    forbidden = set()

    def audit(frame, event, arg):
        if event == 'call':
            name, module = frame.f_code.co_name, frame.f_globals.get('__name__', '')
            if module == 'unittest.case' and name in ('assertEqual', 'assertNotEqual', 'assertIsInstance'):
                assertions[name] += 1
            if module.split('.')[0] == 'pathlib':
                forbidden.add(module + '.' + frame.f_code.co_qualname)
        elif event == 'c_call' and arg in (os.stat, os.lstat):
            forbidden.add(arg.__module__ + '.' + arg.__name__)

    previous = sys.getprofile()
    if 'cls' in vars(case):
        raise ValueError('unexpected per-instance path factory')
    case.cls = NativePath
    try:
        sys.setprofile(audit)
        original()
    finally:
        sys.setprofile(previous)
        del case.cls
        bridge.close()
    if forbidden or bridge.calls != FIELD_CALLS or assertions != FIELD_ASSERTIONS:
        raise AssertionError((forbidden, bridge.calls, assertions))
    body = method_bytes(MODULE, 'PathTest', 'test_stat')
    return {'roles': roles(body='cpython', assertions='cpython', operation='imre', fixture='cpython'),
            'evidence': {'operation': 'native construction, joinpath, stat and individual integer field reads',
                         'fixture': 'original CPython filesystem setup and teardown; Imre holds paths and stat results',
                         'calls': dict(bridge.calls), 'host_assertions': dict(assertions),
                         'whole_body_blocker': failure, 'method_sha256': hashlib.sha256(body).hexdigest(),
                         'opt_level': tools.level}}


def stat_adapters(tools, work):
    def run(case, original):
        ctor, name = case.cls.__name__, case._testMethodName
        if type(case).__name__ != ctor + 'Test' or case.cls.__module__ != 'pathlib':
            raise ValueError(f'unexpected stat fixture: {case.id()}')
        directory = work / case.id()
        directory.mkdir()
        base_class = '(unittest.TestCase)' if name == 'test_stat' else ''
        init_super = '        super().__init__()\n' if base_class else ''
        fixture = f'''import unittest
from pathlib import {ctor}
from os import stat_result
class NativeCase{base_class}:
    base: str
    def __init__(self) -> None:
{init_super}        self.base = {case.base!r}
    def cls(self, name: str) -> {ctor}:
        return {ctor}(name)
'''
        if name == 'test_stat':
            try:
                return native_method(tools, directory, MODULE, 'PathTest', case.id(), fixture,
                                     operation=('imre', 'native stat_result fields'))(case, original)
            except NativeCompileError as exc:
                # Only switch execution roles if the original instance assertion stopped compilation.
                m = re.search(r'case\.py:(\d+):\d+: Imre:', str(exc))
                if not m or 'self.assertIsInstance(' not in (directory / 'case.py').read_text(encoding='utf-8').splitlines()[int(m[1])-1]:
                    raise
                reason = str(exc).replace(str(directory), '<case>').strip()
            return driven_stat(tools, directory, case, original, reason)
        # The native TestCase methods take integers. These complete methods compare
        # stat_result values; a separate native fixture keeps that exact contract.
        fixture += '''    def assertEqual(self, left: stat_result, right: stat_result) -> None:
        if left != right:
            raise AssertionError('different stat results')
    def assertNotEqual(self, left: stat_result, right: stat_result) -> None:
        if left == right:
            raise AssertionError('equal stat results')
'''
        body = (registered_body(case, original, 'PathTest', 'symlinks') if name in SYMLINK
                else method_bytes(MODULE, 'PathTest', name))
        label = 'completed ' + case.id()
        entry = fixture.encode() + body + f'\nNativeCase().{name}()\nprint({label!r})\n'.encode()
        binary = tools.build(entry, directory)
        proc = subprocess.run([binary], capture_output=True, encoding='utf-8', timeout=60)
        if proc.returncode or proc.stdout != label + '\n' or proc.stderr:
            raise AssertionError((case.id(), proc.returncode, proc.stdout, proc.stderr))
        return {'roles': roles(body='imre', assertions='imre', operation='imre', fixture='mixed'),
                'evidence': {'operation': 'native stat/lstat and stat_result tuple comparison',
                             'assertions': 'typed native stat_result equality or inequality',
                             'fixture': 'original CPython setup, symlink gates and teardown; native test instance',
                             'method_sha256': hashlib.sha256(body).hexdigest(), 'opt_level': tools.level}}

    return {f'test.test_pathlib.test_pathlib.{ctor}Test.{name}': run
            for ctor in ('Path', 'PosixPath') for name in METHODS}
