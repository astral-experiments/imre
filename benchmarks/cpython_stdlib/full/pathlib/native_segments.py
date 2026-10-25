"""Run original constructor failure methods with the native path flavour under test."""
from collections import Counter
import hashlib
import os
import pathlib
import re
import sys

from native_traversal import Bridge, MODULE, PREFIX
from suite_evidence import roles
from suite_native import NativeCompileError, method_bytes, native_method


BYTE_CALLS = {'b': 1, 'bs': 1, 'sb': 1}
BYTE_ASSERTIONS = {'assertRaisesRegex': 3}
SEGMENT_IDS = {
    PREFIX + ctor + 'Test.' + name
    for name, ctors in [('test_passing_kwargs_errors', ('Path', 'PosixPath')),
                        ('test_bytes_exc_message', ('Path', 'PosixPath', 'PurePath', 'PurePosixPath', 'PureWindowsPath'))]
    for ctor in ctors
}


def segment_adapters(tools, work):
    def run(case, original):
        ctor, name = case.cls.__name__, case._testMethodName
        if (case.cls is not getattr(pathlib, ctor) or type(case).__name__ != ctor + 'Test'
                or 'cls' in vars(case)):
            raise ValueError(f'unexpected original fixture for {case.id()}')
        directory = work / case.id()
        directory.mkdir()
        fixture = f'''import unittest
from pathlib import {ctor}
class NativeCase(unittest.TestCase):
'''
        if name == 'test_passing_kwargs_errors':
            # Accept the fixture keyword so only the actual constructor can reject it.
            fixture += f'''    def cls(self, *, foo: str) -> {ctor}:
        return {ctor}(foo=foo)
'''
            return native_method(tools, directory, MODULE, 'PathTest', case.id(), fixture,
                                 operation=('imre', 'reject an unknown path constructor keyword'))(case, original)
        fixture += f'''    def cls(self, first: str | bytes, second: str | bytes | None = None) -> {ctor}:
        if second is None:
            return {ctor}(first)
        return {ctor}(first, second)
'''
        description = 'reject a bytes argument in each of the three original constructor calls'
        try:
            return native_method(tools, directory, MODULE, 'PurePathTest', case.id(), fixture,
                                 operation=('imre', description))(case, original)
        except NativeCompileError as exc:
            line = re.search(r'case\.py:(\d+):\d+: Imre: ([^\n]+)', str(exc))
            if not line:
                raise
            source = (directory / 'case.py').read_text(encoding='utf-8').splitlines()[int(line[1]) - 1].strip()
            # After #995 the body enters its regex context, then reaches the
            # saved constructor's omitted second argument. Other failures surface.
            if (source, line[2]) not in {
                ('with self.assertRaisesRegex(TypeError, message):', 'unknown class method'),
                ("P(b'a')", 'indirect calls require the full positional Callable signature; '
                             'keywords and omitted defaults are unsupported'),
            }:
                raise
            reason = str(exc).replace(str(directory), '<case>').strip()

        driver = pathlib.Path(__file__).with_name('segment_driver.py').read_bytes().replace(
            b'from pathlib import Path', f'from pathlib import {ctor} as Path'.encode())
        bridge = Bridge(tools.build(driver, directory), directory, os.getcwd())

        def construct(*parts):
            signature = ''.join({str: 's', bytes: 'b'}[type(part)] for part in parts)
            return bridge.call(signature, *parts)

        original_code = original.__func__.__code__
        host, forbidden = Counter(), set()

        def audit(frame, event, arg):
            if event != 'call':
                return
            owner, name = frame.f_globals.get('__name__', ''), frame.f_code.co_name
            if owner.split('.')[0] == 'pathlib':
                forbidden.add(frame.f_code.co_qualname)
            if (frame.f_back and frame.f_back.f_code is original_code
                    and owner == 'unittest.case' and name == 'assertRaisesRegex'):
                host[name] += 1

        previous = sys.getprofile()
        case.cls = construct
        try:
            sys.setprofile(audit)
            original()
        finally:
            sys.setprofile(previous)
            del case.cls
            bridge.close()
        if forbidden or bridge.calls != BYTE_CALLS or host != BYTE_ASSERTIONS:
            raise AssertionError((case.id(), forbidden, bridge.calls, host))
        body = method_bytes(MODULE, 'PurePathTest', name)
        return {'roles': roles(body='cpython', assertions='cpython', operation='imre', fixture='cpython'),
                'evidence': {'operation': description, 'native_calls': dict(bridge.calls), 'host_calls': dict(host),
                             'fixture': 'original CPython instance and hooks; all constructor calls run in Imre',
                             'method_sha256': hashlib.sha256(body).hexdigest(), 'whole_body_blocker': reason,
                             'opt_level': tools.level}}

    return dict.fromkeys(SEGMENT_IDS, run)
