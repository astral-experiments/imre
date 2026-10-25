"""Run original archive methods against native writers, readers, dispatch and real files."""
import argparse
import ast
from contextlib import chdir, ExitStack
import errno
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path[:0] = [str(HERE), str(STDLIB)]
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_evidence import roles
from suite_native import NativeCompileError, Toolchain, native_method, run_in_fixtures
import archive_registry_case
import archive_packer_case

DRIVER = '''import sys
import shutil
def optional(index: int) -> str | None:
    if sys.argv[index] == 'present':
        return sys.argv[index + 1]
    return None
try:
    if sys.argv[1] == 'make':
        result = shutil.make_archive(sys.argv[2], sys.argv[3], root_dir=optional(4),
                base_dir=optional(6), dry_run=sys.argv[8] == '1', owner=optional(9), group=optional(11))
        print('ok')
        print(repr(result))
    else:
        shutil.unpack_archive(sys.argv[2], optional(3), optional(5), filter=optional(7))
        print('ok')
        print('None')
except shutil.ReadError as e:
    print('ReadError')
    print(repr(str(e)))
except ValueError as e:
    print('ValueError')
    print(repr(str(e)))
except TypeError as e:
    print('TypeError')
    print(repr(str(e)))
'''
ROOT_ERRORS = {"FileNotFoundError": FileNotFoundError, "NotADirectoryError": NotADirectoryError}
ROOT_CASES = {"test_make_tarfile_rootdir_nodir", "test_make_zipfile_rootdir_nodir"}
for name in ROOT_ERRORS:
    DRIVER += f'''except {name} as e:
    print("{name}")
    print(repr(e.errno))
    print(repr(e.strerror))
    print(repr(e.filename))
    print(repr(e.filename2))
    print(repr(e))
    print(repr(str(e)))
'''

def root_error(status, data):
    number, description, first, second, representation, message = data.splitlines()
    number, description, first, second, message = map(ast.literal_eval, (number, description, first, second, message))
    cls = ROOT_ERRORS[status]
    error = cls(number, description, first) if second is None else cls(number, description, first, None, second)
    assert repr(error) == representation and str(error) == message
    return error


def make_archive_fixture(case):
    return ('import os.path\nimport unittest\nfrom shutil import make_archive\n\n'
            'class NativeCase(unittest.TestCase):\n'
            f'    def mkdtemp(self) -> str:\n        return {case.mkdtemp()!r}\n')


def executors(main, tools, folder, *, with_roles=False, blockers=None):
    guard = folder / 'chdir.so'
    subprocess.run([tools.clang, '-shared', '-fPIC', '-Wall', '-Werror', HERE/'archive_chdir.c', '-o', guard], check=True)
    binary = tools.build(DRIVER.encode(), folder)
    registry = folder / "registry"
    registry.mkdir()
    registry_binary = tools.build(archive_registry_case.SOURCE.encode(), registry)
    packing = folder / "packing"
    packing.mkdir()
    blocked = {} if blockers is None else blockers
    try:
        pack_binary = tools.build(archive_packer_case.SOURCE.encode(), packing)
    except NativeCompileError as exc:
        blocked.update(dict.fromkeys(archive_packer_case.METHODS, str(exc)))
    suite = load_suite('test.test_shutil')
    def execute(case, original):
        name = case._testMethodName
        if name in blocked:
            return {"status": "blocked", "reason": blocked[name]}
        if name in archive_packer_case.METHODS:
            detail = archive_packer_case.run(main, original, [pack_binary], guard)
            counts = detail["calls_by_operation"]
            detail.update(native_calls=sum(counts.values()), opt_level=tools.level,
                          fixture="original host body, assertions and callbacks; integer tuple rows are transported as native lists; chdir events originate in the native process")
            if with_roles:
                return {"roles": roles(body="cpython", assertions="cpython", operation="mixed", fixture="cpython"),
                        "evidence": dict(operation=", ".join("shutil." + op for op in counts), **detail)}
            return {"status": "mixed", **detail}
        if name == "test_unpack_registry":
            counts, callback_types = archive_registry_case.run(main, original, [registry_binary])
            assert callback_types == [["str", "str", "int"]]
            detail = {"calls_by_operation": counts, "native_calls": sum(counts.values()),
                      "callback_argument_types": callback_types, "opt_level": tools.level,
                      "fixture": "original CPython TestArchives fixture, callback closure and assertions; one native process owns the registry and suffix dispatch"}
            if with_roles:
                return {"roles": roles(body="cpython", assertions="cpython", operation="mixed", fixture="cpython"),
                        "evidence": dict(operation=", ".join("shutil." + op for op in counts), **detail)}
            return {"status": "mixed", **detail}
        native_blocker = {}
        if name == 'test_make_archive':
            routine = folder/'unchanged-make-archive'
            routine.mkdir()
            try:
                run = native_method(tools, routine, Path(main.__file__), 'TestArchives', case.id(),
                                    make_archive_fixture(case), operation=('imre', 'shutil.make_archive'))
            except NativeCompileError as exc:
                native_blocker['native_body_blocker'] = str(exc).strip().splitlines()[-1].split('Imre: ')[-1]
            else:
                record = run(case, original)
                return record if with_roles else {'status':'native_body_host_fixture', **record}
        counts = {"make_archive": 0, "unpack_archive": 0}
        conversions, guarded, errors = {}, 0, []
        def path(value):
            if not isinstance(value, (str, bytes)):
                cls = type(value).__name__
                conversions[cls] = conversions.get(cls, 0) + 1
            return os.fspath(value)

        def optional(value):
            return ['none', ''] if value is None else ['present', path(value)]

        def invoke(op, *argv):
            nonlocal guarded
            counts[op] += 1
            with tempfile.TemporaryFile() as attempts:
                env = os.environ.copy()
                fds = ()
                if isinstance(main.os.chdir, Mock):
                    guarded += 1
                    env.update(LD_PRELOAD=str(guard), IMRE_SHUTIL_TEST_CHDIR_FD=str(attempts.fileno()))
                    fds = (attempts.fileno(),)
                result = subprocess.run([binary, *argv], env=env, pass_fds=fds, capture_output=True, text=True, timeout=90)
                attempts.seek(0)
                for attempted in attempts.read().split(b'\0')[:-1]:
                    main.os.chdir(os.fsdecode(attempted))
            if result.returncode or result.stderr:
                raise AssertionError(f"native {op} ({result.returncode}): {result.stderr}")
            status, data = result.stdout.rstrip('\n').split('\n', 1)
            if status in ROOT_ERRORS:
                error = root_error(status, data)
                if name in ROOT_CASES:
                    errors.append([type(error).__name__, error.errno, error.filename == argv[4], error.filename2 is None])
                raise error
            value = ast.literal_eval(data)
            if status != 'ok':
                raise {'ReadError': main.shutil.ReadError, 'ValueError': ValueError, 'TypeError': TypeError}[status](value)
            return value

        def make(base_name, format, root_dir=None, base_dir=None, verbose=0, dry_run=0, owner=None, group=None, logger=None):
            assert logger is None and not verbose
            return invoke('make_archive', 'make', path(base_name), format, *optional(root_dir), *optional(base_dir),
                          str(int(dry_run)), *optional(owner), *optional(group))

        def unpack(filename, extract_dir=None, format=None, *, filter=None):
            return invoke('unpack_archive', 'unpack', path(filename), *optional(extract_dir), *optional(format), *optional(filter))

        with ExitStack() as stack:
            stack.enter_context(patch.object(main, 'make_archive', make))
            stack.enter_context(patch.object(main, 'unpack_archive', unpack))
            original()
        assert sum(counts.values()) > 0
        detail = {"guarded_native_calls": guarded}
        if name in ROOT_CASES:
            assert errors == [['FileNotFoundError', errno.ENOENT, True, True],
                              ['NotADirectoryError', errno.ENOTDIR, True, True]] * 2, errors
            detail["root_errors"] = errors
        if conversions:
            detail.update(host_pathlike_conversions=sum(conversions.values()), host_path_types=conversions,
                          host="CPython evaluated the path protocol before the native archive call")
        if with_roles:
            return {"roles": roles(body="cpython", assertions="cpython", operation="mixed" if conversions else "imre", fixture="cpython"),
                    "evidence": dict(operation=", ".join("shutil."+op for op, count in counts.items() if count),
                                     calls_by_operation=counts, native_calls=sum(counts.values()), opt_level=tools.level,
                                     fixture="original CPython archive inputs, tool guards, readers, assertions and cleanup",
                                     **detail)}
        return {"status": "mixed" if conversions else "host_driven_native", "operations": counts, **detail, **native_blocker}
    return {c.id(): execute for c in cases(suite) if isinstance(c, main.TestArchives)
            and (not with_roles or c._testMethodName not in blocked)}


def run_level(tools, main, folder):
    suite = load_suite('test.test_shutil')
    selected = executors(main, tools, folder)
    good, failures, skips = run_in_fixtures(suite, selected)
    rows = {n: v[0] for n,v in good.items() if v}
    rows.update({n: {"status":"failed", "reason":msg} for n,msg in failures})
    rows.update({n: {"status":"reference_skip", "reason":msg} for n,msg in skips})
    assert rows.keys() == selected.keys() and len(rows) == 28
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ('imre', 'runtime', 'clang', 'result'):
        parser.add_argument('--'+arg, type=lambda p: Path(p).resolve(), required=True)
    parser.add_argument('--cpython-source', type=Path)
    args = parser.parse_args()
    if sys.implementation.name != 'cpython' or sys.version_info[:3] != (3,14,5) or sys.platform != 'linux':
        parser.error('requires the pinned test-enabled Linux CPython interpreter')
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / 'Lib'))
    manifest = json.loads((HERE/'UPSTREAM.json').read_text())
    verify_files(STDLIB/'upstream', manifest['files'])
    verify_interpreter_tests(manifest)
    import test
    source = STDLIB/'upstream/Lib/test'
    test.__path__.insert(0, str(source))
    levels = {}
    with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
        root = Path(tmp)
        from test import test_shutil
        assert Path(test_shutil.__file__).resolve() == source/'test_shutil.py'
        for level in (0,3):
            folder=root/str(level)
            folder.mkdir()
            levels[level] = run_level(Toolchain(args.imre,args.runtime,args.clang,level), test_shutil, folder)
    def digest(path):
        with path.open('rb') as handle:
            return hashlib.file_digest(handle, 'sha256').hexdigest()
    args.result.write_text(json.dumps({'cpython_commit':manifest['commit'],
                          'compiler_sha256':digest(args.imre), 'runtime_sha256':digest(args.runtime),
                          'optimization_levels':levels},indent=2)+'\n')
    return int(any(v['status']=='failed' for rows in levels.values() for v in rows.values()))


if __name__ == '__main__':
    sys.exit(main())
