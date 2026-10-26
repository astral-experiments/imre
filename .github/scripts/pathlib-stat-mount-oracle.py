import os, sys, tempfile, unittest
from contextlib import chdir
from pathlib import Path
root=sys.argv[1] if len(sys.argv)>1 else None
with tempfile.TemporaryDirectory(prefix='pl-oracle-', dir=root) as tmp, chdir(tmp):
    from test.test_pathlib.test_pathlib import PathTest, PosixPathTest
    for cls in (PathTest,PosixPathTest):
        case=cls('test_stat')
        case.setUp()
        try:
            devices={name:os.stat(os.path.join(case.base,name)).st_dev for name in ('fileA','dirB/fileB','dirC')}
            print(cls.__name__, 'fixture', tmp, 'st_dev',devices, flush=True)
            try:
                case.test_stat()
            except AssertionError as error:
                assert len(set(devices.values()))>1, error
                print(cls.__name__, 'original CPython assertion:', error, flush=True)
            else:
                assert len(set(devices.values()))==1
                print(cls.__name__, 'original CPython test_stat passed', flush=True)
        finally:
            case.tearDown()
