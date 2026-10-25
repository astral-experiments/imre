"""Run Path.info methods inside the original Linux filesystem fixtures."""

import hashlib
from pathlib import Path
import subprocess

from native_traversal import registered_body
from suite_evidence import roles
from suite_native import native_method

HERE = Path(__file__).resolve().parent
MODULE = HERE.parents[1] / 'upstream/Lib/test/test_pathlib/test_pathlib.py'
METHODS = ('test_info_exists_caching', 'test_info_is_dir_caching', 'test_info_is_file_caching')
SYMLINK_METHODS = ('test_info_is_symlink_caching', 'test_readlink')


def metadata_adapters(tools, work):
    def run(case, original):
        ctor = case.cls.__name__
        if type(case).__name__ != ctor + 'Test' or case.cls.__module__ != 'pathlib':
            raise ValueError(f'unexpected concrete pathlib fixture: {case.id()}')
        directory = work / case.id()
        directory.mkdir()
        fixture = f'''import unittest
from pathlib import {ctor}
class NativeCase(unittest.TestCase):
    base: str
    def __init__(self) -> None:
        super().__init__()
        self.base = {case.base!r}
    def cls(self, name: str) -> {ctor}:
        return {ctor}(name)
'''
        if case._testMethodName in SYMLINK_METHODS:
            body = registered_body(case, original, 'PathTest', 'symlinks')
            readlink = case._testMethodName == 'test_readlink'
            if readlink:
                fixture += f'''    def assertEqual(self, first: {ctor}, second: {ctor}) -> None:
        if first != second:
            raise AssertionError('readlink returned a different target')
'''
            label = 'completed ' + case.id()
            entry = fixture.encode() + body + f'\nNativeCase().{case._testMethodName}()\nprint({label!r})\n'.encode()
            result = subprocess.run([tools.build(entry, directory)], capture_output=True, timeout=60)
            if result.returncode or result.stdout != (label + '\n').encode() or result.stderr:
                raise AssertionError((case.id(), result.returncode, result.stdout, result.stderr))
            return {'roles': roles(body='imre', assertions='imre', operation='imre', fixture='mixed'),
                    'evidence': {'operation': ('four stored link targets and OSError for an ordinary file' if readlink
                                               else 'Path.info.is_symlink cache across native symlink_to and unlink'),
                                 'unconditional_assertions': ({'assertEqual': 4, 'assertRaises': 1} if readlink
                                                              else {'assertFalse': 2, 'assertTrue': 2}),
                                 'fixture': 'original needs_symlinks registry and setUp; native test instance',
                                 'method_sha256': hashlib.sha256(body).hexdigest(), 'opt_level': tools.level}}
        return native_method(tools, directory, MODULE, 'PathTest', case.id(), fixture,
                             operation=('imre', 'Path.info queries before and after a native filesystem mutation'))(case, original)

    return {f'test.test_pathlib.test_pathlib.{cls}.{name}': run
            for cls in ('PathTest', 'PosixPathTest') for name in METHODS + SYMLINK_METHODS}
