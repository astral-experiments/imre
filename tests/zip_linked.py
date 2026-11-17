#!/usr/bin/env python3
"""Exercise finite ZIP imports after withdrawing their build-time archive."""
import argparse
import os
import subprocess
import tempfile
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile


def call(argv, *, cwd, env=None, timeout=180):
    return subprocess.run([str(x) for x in argv], cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)


def require(result, expected):
    assert (result.returncode, result.stdout, result.stderr) == expected, (result.args, result.returncode, result.stdout, result.stderr)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--python', default=os.sys.executable)
    args = parser.parse_args()
    compiler, runtime = args.compiler.resolve(), args.runtime.resolve()
    with tempfile.TemporaryDirectory(prefix='imre-linked-zip-') as directory:
        root = Path(directory)
        bundle = root / 'modules.zip'
        with ZipFile(bundle, 'w', compression=ZIP_DEFLATED) as archive:
            archive.writestr('sample/__init__.py', 'from .leaf import value\nprint("init")\n')
            archive.writestr('sample/leaf.py', 'value = 42\n')
            archive.writestr('portion/', '')
            archive.writestr('portion/child.py', 'answer = 17\n')
            archive.writestr('sample/data.txt', 'resource')
        alias = root / 'alias.zip'
        alias.symlink_to(bundle)
        entry = root / 'main.py'
        entry.write_text("""\
import sample
import sample as again
from portion import child
import zipimport
assert sample is again
assert sample.__spec__.loader is sample.__loader__
assert type(sample.__loader__) is zipimport.zipimporter
assert sample.__spec__.origin == sample.__file__
assert sample.__file__.endswith("/modules.zip/sample/__init__.py")
assert sample.__path__[0].endswith("/modules.zip/sample")
print(sample.value, child.answer)
import sys
if len(sys.argv) > 1:
    try:
        sample.__loader__.get_data(sample.__path__[0] + "/data.txt")
    except OSError:
        print("resource unavailable")
    else:
        raise AssertionError("withdrawn resource opened")
""")
        env = dict(os.environ, PYTHONPATH=str(bundle), PYTHONDONTWRITEBYTECODE='1')
        expected = (0, 'init\n42 17\n', '')
        require(call([args.python, '-B', entry], cwd=root, env=env), expected)
        images = []
        for publisher, flags, levels in [('split', [], (0, 3)), ('whole', ['--whole-program'], (0,))]:
            for level in levels:
                image = root / f'{publisher}-O{level}'
                roots = ['--extension-path', bundle]
                if publisher == 'split' and level == 0:
                    roots += ['--extension-path', bundle, '--extension-path', alias]
                result = call([compiler, 'build', entry, *roots, '--runtime', runtime, '--no-cache', '-q', '--opt-level', level, *flags, '-o', image], cwd=root)
                require(result, (0, '', ''))
                images.append(image)
        entry.rename(root / 'main.withdrawn')
        bundle.rename(root / 'modules.withdrawn')
        for image in images:
            require(call([image], cwd=root, env={'PATH': '/nonexistent'}), expected)
        require(call([images[0], '--resource'], cwd=root, env={'PATH': '/nonexistent'}), (0, 'init\n42 17\nresource unavailable\n', ''))
        (root / 'main.withdrawn').rename(entry)
        (root / 'modules.withdrawn').rename(bundle)
        # Application-owned imports retain enforced annotation contracts. An unmarked
        # third-party extension archive retains ordinary Python annotation semantics.
        (root / 'helper.py').write_text('status: int = True\n')
        entry.write_text('import helper\n')
        for flags in ([], ['--whole-program']):
            image = root / ('poison-whole' if flags else 'poison-split')
            result = call([compiler, 'build', entry, '--extension-path', bundle, '--runtime', runtime, '--no-cache', '-q', *flags, '-o', image], cwd=root)
            assert result.returncode != 0 and 'helper.py' in result.stderr and not image.exists(), (result.stderr, image.exists())
        entry.write_text('status: int = True\n')
        image = root / 'poison-entry'
        result = call([compiler, 'build', entry, '--runtime', runtime, '--no-cache', '-q', '-o', image], cwd=root)
        assert result.returncode != 0 and 'main.py' in result.stderr and not image.exists(), (result.stderr, image.exists())
    print('linked ZIP imports: stock, split O0/O3, whole O0, and poison controls passed')


if __name__ == '__main__':
    main()
