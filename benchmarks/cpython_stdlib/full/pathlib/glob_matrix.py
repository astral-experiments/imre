"""Compare ordered traversal, duplicates, and exception messages on the same POSIX tree."""

import argparse
import ast
from contextlib import chdir
from itertools import product
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_native import Toolchain

DRIVER = b"""from pathlib import Path
import sys

def sensitivity(text: str) -> bool | None:
    if text == 'none':
        return None
    return text == 'true'

root = Path(sys.argv[1])
pattern = sys.argv[2]
case = sensitivity(sys.argv[3])
follow = sys.argv[4] == 'true'
try:
    if sys.argv[5] == 'rglob':
        for p in root.rglob(pattern, case_sensitive=case, recurse_symlinks=follow):
            print(repr(str(p)))
    else:
        for p in root.glob(pattern, case_sensitive=case, recurse_symlinks=follow):
            print(repr(str(p)))
except ValueError as error:
    print('ValueError:' + str(error))
except NotImplementedError as error:
    print('NotImplementedError:' + str(error))
"""

PATTERNS = ('', '.', './', '/', 'foo', 'Foo', 'f?o', 'foo/', 'foo/..', 'a/../foo',
            '*', '*/', '**', '**/', '**/*/', '**/**', '**/**/', '*/**', '**/foo', '**/F*',
            '**/*/**', '*/*/..', '../root/*', '*//./foo', '*/**/../*', '[ai]*', '**/[i]',
            '**/[[]x]', '**/*a*', 'li*/**/', '**/link/**', 'a*', '**/..', '**/*/*',
            '**/*/**/foo', './**/./', '*/*/', 't**o', '...', 'bad[')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ('imre', 'runtime', 'clang'):
        parser.add_argument('--' + option, type=Path, required=True)
    parser.add_argument('--opt-level', type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.platform == 'win32':
        parser.error('use CPython 3.14.5 on POSIX')
    tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), args.opt_level)
    with tempfile.TemporaryDirectory(prefix='pl-') as directory, chdir(directory):
        work = Path(directory)
        binary = tools.build(DRIVER, work)
        tree = Path('root')
        tree.mkdir()
        for name in ('a', 'a/b', 'c', 'δ'):
            (tree / name).mkdir()
        for name in ('foo', 'a/foo', 'a/b/Foo', 'a/.hidden', 'c/bar', 'δ/İ', 'c/new\nline', 'δ/[x]'):
            (tree / name).touch()
        for name, target in (('link', 'a'), ('a/toC', '../c'), ('broken', 'absent'), ('loop', 'loop')):
            (tree / name).symlink_to(target)
        total = 0
        for root, pattern, case, follow, method in product(
                (tree, tree / 'foo', tree / 'missing', Path('.')), PATTERNS,
                (None, False, True), (False, True), ('glob', 'rglob')):
            try:
                expected = ''.join(repr(str(p)) + '\n' for p in getattr(root, method)(
                    pattern, case_sensitive=case, recurse_symlinks=follow))
            except (ValueError, NotImplementedError) as error:
                expected = f'{type(error).__name__}:{error}\n'
            result = subprocess.run([binary, str(root), pattern, str(case).lower(),
                                     str(follow).lower(), method], capture_output=True, text=True, timeout=10)
            if result.returncode or result.stderr or result.stdout != expected:
                raise AssertionError((root, pattern, case, follow, method, result, expected))
            total += 1
        # The kernel can stop resolving a directory symlink loop at different
        # depths on repeated calls, even for CPython on the very same tree.
        # Compare through the first back edge, as the original pathlib tests do.
        cycle = Path('cycle')
        (cycle / 'd').mkdir(parents=True)
        (cycle / 'd' / 'leaf').touch()
        (cycle / 'd' / 'back').symlink_to('../d')
        requests = (('glob', '**'), ('glob', '**/'), ('glob', '**/leaf'),
                    ('rglob', '*'), ('rglob', '*/'), ('rglob', 'leaf'))
        for method, pattern in requests:
            expected = list(getattr(cycle, method)(pattern, recurse_symlinks=True))
            result = subprocess.run([binary, str(cycle), pattern, 'none', 'true', method],
                                    capture_output=True, text=True, timeout=10, check=True)
            actual = [Path(ast.literal_eval(line)) for line in result.stdout.splitlines()]
            if result.stderr or not all(any(p.parts.count('back') > 1 for p in paths)
                                        for paths in (actual, expected)):
                raise AssertionError((method, pattern, result))
            if [p for p in actual if p.parts.count('back') <= 1] != [
                    p for p in expected if p.parts.count('back') <= 1]:
                raise AssertionError((method, pattern, actual, expected))
    print(f'O{args.opt_level}: all {total} ordered glob results and {len(requests)} '
          'bounded symlink-cycle comparisons matched CPython')


if __name__ == '__main__':
    main()
