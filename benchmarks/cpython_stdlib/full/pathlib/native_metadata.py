"""Run Path.info methods inside the original Linux filesystem fixtures."""

from pathlib import Path

from suite_native import native_method

HERE = Path(__file__).resolve().parent
MODULE = HERE.parents[1] / 'upstream/Lib/test/test_pathlib/test_pathlib.py'
METHODS = ('test_info_exists_caching', 'test_info_is_dir_caching', 'test_info_is_file_caching')


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
        return native_method(tools, directory, MODULE, 'PathTest', case.id(), fixture,
                             operation=('imre', 'Path.info queries before and after a native filesystem mutation'))(case, original)

    return {f'test.test_pathlib.test_pathlib.{cls}.{name}': run
            for cls in ('PathTest', 'PosixPathTest') for name in METHODS}
