"""Compare walk order, caller edits, and the resulting path values with CPython 3.14.5."""

import argparse
from itertools import product
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_native import Toolchain


DRIVER = r'''
import sys
from pathlib import Path

raw = sys.argv[1]
if raw == '::nul':
    raw = 'bad\0root'
down = sys.argv[2] == '1'
follow = sys.argv[3] == '1'
mode = sys.argv[4]
i = 0
try:
    for here, dirs, names in Path(raw).walk(down, None, follow):
        print(repr(str(here)))
        print(repr(dirs))
        print(repr(names))
        print(repr(here.name))
        print(repr(str(here.parent)))
        print(here == Path(str(here)))
        if not i:
            if mode != 'keep':
                while dirs:
                    dirs.pop()
            if mode == 'duplicates':
                dirs.append('a')
                dirs.append('a')
            if mode == 'absolute':
                dirs.append(sys.argv[5])
            if mode == 'raw':
                dirs.append('./a')
                dirs.append('a//.')
                dirs.append('')
            if mode == 'links':
                dirs.append('link')
                dirs.append('absent')
                dirs.append('file')
        i += 1
        if i == 24:
            break
except ValueError as error:
    print(str(error))
'''.encode()


def run(command, cwd):
    completed = subprocess.run(command, cwd=cwd, capture_output=True, timeout=10, check=True)
    if completed.stderr:
        raise AssertionError((command, completed.stderr))
    return completed.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('imre', 'runtime', 'clang'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--opt-level', type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.platform == 'win32':
        parser.error('use CPython 3.14.5 on POSIX')
    tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), args.opt_level)
    with tempfile.TemporaryDirectory(prefix='pl-walk-') as tmp:
        work = Path(tmp)
        binary = tools.build(DRIVER, work)
        fixture = work / 'fixture'
        root = fixture / 'root'
        for name in ('a', 'a/nest', 'branch', 'ευ', 'newline\nname'):
            (root / name).mkdir(parents=True)
        for name in ('file', 'branch/file', '.hidden', 'a/z'):
            (root / name).touch()
        (root / 'link').symlink_to('a')
        (root / 'broken').symlink_to('not-here')
        count = 0
        for case in product(('.', '', 'root', 'root/.', 'root/a', 'root/link', 'absent', 'root/file', '::nul'),
                            ('0', '1'), ('0', '1'), ('keep', 'prune', 'duplicates', 'absolute', 'raw', 'links')):
            arguments = [*case, str(root / 'a')]
            expected = run([sys.executable, '-B', work / 'case.py', *arguments], fixture)
            actual = run([binary, *arguments], fixture)
            if actual != expected:
                raise AssertionError((case, expected, actual))
            count += 1
        print(f'O{args.opt_level}: {count} walks matched the reference')


if __name__ == '__main__':
    main()
