"""Compile complete lexical methods with their actual POSIX or Windows path flavor."""
import argparse
import ast
from collections import defaultdict
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

BENCHMARK = Path(__file__).resolve().parents[2]
PACKAGE = BENCHMARK / "upstream/Lib/test/test_pathlib"
sys.path.insert(0, str(BENCHMARK))
from upstream import verify_upstream

# Fixture classes are defined in the upstream children. PathTest and PosixPathTest
# are distinct IDs even when they inherit the same method and use the same flavor.
COMMON = ('PurePath', 'PurePosixPath', 'PureWindowsPath', 'Path', 'PosixPath')
POSIX = ('PurePath', 'PurePosixPath', 'Path', 'PosixPath')
WINDOWS = ('PureWindowsPath',)


def groups():
    result = defaultdict(list)

    def add(module, owner, flavors, suffix, methods, expected):
        for flavor in flavors:
            for method in methods.split():
                fixture = flavor + suffix
                result[module, owner, fixture, flavor, expected].append('test_' + method)

    def common(flavors, methods, expected='str'):
        add('test_pathlib', 'PurePathTest', flavors, 'Test', methods, expected)

    common(COMMON, 'drive_common root_common name_empty stem_empty as_posix_common')
    common(COMMON, 'div_nested join_nested', 'path')
    common(COMMON, 'full_match_case_sensitive match_common')
    common(POSIX, 'root_posix is_absolute_posix match_posix')
    common(POSIX, 'eq_posix', 'path')
    common(WINDOWS, 'drive_windows root_windows str_windows match_windows')
    add('test_join', 'JoinTestBase', ('Path', 'PurePath'), 'JoinTest', 'name stem suffix full_match', 'str')
    add('test_join', 'JoinTestBase', ('Path', 'PurePath'), 'JoinTest', 'parent', 'path')
    add('test_join_posix', 'JoinTestBase', ('PosixPath', 'PurePosixPath'), 'JoinTest', 'join div', 'path')
    add('test_join_windows', 'JoinTestBase', WINDOWS, 'JoinTest', 'str anchor name stem suffix', 'str')
    add('test_join_windows', 'JoinTestBase', WINDOWS, 'JoinTest', 'join div parent', 'path')
    return result


def native_source(*, dispatch=False):
    source = bytearray(b'from pathlib import Path, PosixPath, PurePath, PurePosixPath, PureWindowsPath\n')
    if dispatch:
        source.extend(b'import sys\n')
    runs, rows = [], {}
    for (module, owner, fixture, ctor, expected), names in groups().items():
        data = (PACKAGE / (module + '.py')).read_bytes()
        cls = next(n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == owner)
        methods = {m.name: m for m in cls.body if isinstance(m, ast.FunctionDef)}
        name = fixture + '_' + expected
        ty = ctor if expected == 'path' else expected
        sep = '\\' if ctor == 'PureWindowsPath' else '/'
        source.extend(f'''
class {name}:
    checks: int
    sep: str
    def __init__(self) -> None:
        self.checks = 0
        self.sep = {sep!r}
    def cls(self, value: str) -> {ctor}:
        return {ctor}(value)
    def assertTrue(self, value: bool) -> None:
        if not value:
            raise AssertionError('expected True')
        self.checks += 1
    def assertFalse(self, value: bool) -> None:
        if value:
            raise AssertionError('expected False')
        self.checks += 1
    def assertEqual(self, left: {ty}, right: {ty}) -> None:
        if left != right:
            raise AssertionError('expected equal values')
        self.checks += 1
    def assertNotEqual(self, left: {ty}, right: {ty}) -> None:
        if left == right:
            raise AssertionError('expected distinct values')
        self.checks += 1
'''.encode())
        for method in names:
            m = methods[method]
            for decorator in m.decorator_list:
                if ast.unparse(decorator) != ('needs_windows' if ctor == 'PureWindowsPath' else 'needs_posix'):
                    raise ValueError(f'unexpected fixture precondition: {method}')
            body = b''.join(data.splitlines(keepends=True)[m.lineno - 1:m.end_lineno])
            source.extend(b'\n' + body)
            cid = f'test.test_pathlib.{module}.{fixture}.{method}'
            rows[cid] = {'method_sha256': hashlib.sha256(body).hexdigest()}
            run = f'c_{name} = {name}()\nc_{name}.{method}()\nprint(f"{cid} {{c_{name}.checks}}")\n'
            if dispatch:
                run = f'if sys.argv[1] == {cid!r}:\n' + ''.join('    ' + line for line in run.splitlines(True))
            runs.append(run)
    source.extend('\n'.join(runs).encode())
    return source, rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ('imre', 'runtime', 'clang', 'result'):
        parser.add_argument('--' + arg, required=True, type=Path)
    parser.add_argument('--opt-level', choices=('0', '3'), required=True)
    args = parser.parse_args()
    if os.name != 'posix' or sys.version_info[:3] != (3, 14, 5):
        parser.error('use the CPython 3.14.5 POSIX reference')
    report = {'cpython_commit': verify_upstream(), 'opt_level': int(args.opt_level), 'execution': 'native-body-and-operations'}
    source, rows = native_source()
    with tempfile.TemporaryDirectory() as tmp:
        script, binary = Path(tmp) / 'lexical.py', Path(tmp) / 'lexical'
        script.write_bytes(source)
        expected = subprocess.check_output([sys.executable, '-I', '-B', script], text=True)
        for line in expected.splitlines():
            cid, checks = line.rsplit(' ', 1)
            if int(checks) <= 0 or 'assertions' in rows[cid]:
                raise ValueError(line)
            rows[cid]['assertions'] = int(checks)
        if any('assertions' not in row for row in rows.values()):
            raise ValueError('missing reference assertions')
        subprocess.run([args.imre.resolve(), 'build', script, '--no-cache', '--opt-level', args.opt_level,
                        '--runtime', args.runtime.resolve(), '--clang', args.clang.resolve(), '-o', binary], check=True, timeout=240)
        actual = subprocess.run([binary], capture_output=True, text=True, timeout=90)
        if actual.returncode or actual.stdout != expected or actual.stderr:
            raise AssertionError(f'last native output: {actual.stdout.splitlines()[-2:]}; {actual.stderr}')
    report['passed'] = rows
    args.result.write_text(json.dumps(report, indent=2) + '\n')
    print(f'{len(rows)} native methods; {sum(r["assertions"] for r in rows.values())} assertions')


if __name__ == '__main__':
    main()
