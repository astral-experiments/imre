#!/usr/bin/env python3
"""Exercise checked named entry selection from an installed code archive."""
import argparse
import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from zipfile import ZIP_STORED, ZipFile


def archive(path, poison=None, special=None):
    sources = {
        'sample/__init__.py': 'from .leaf import value\nprint("init")\n',
        'sample/leaf.py': 'value = 42\n',
        'sample/flags.py': 'flag = True\n',
        'sample/dead.py': 'status: int = True\n',
        'sample/literal.py': 'value = 1\n',
        'sample/literal_two.py': 'value = 2\n',
        'sample/inner/__init__.py': 'print("inner")\n',
        'sample/inner/__main__.py': (
            'from sample import value\nimport sample, importlib\n'
            'if False:\n    import sample.dead\n    __import__("sample.dead")\n    importlib.import_module(unknown)\n'
            '__import__("sample.literal")\nimportlib.import_module("sample.literal_two")\n'
            'assert sample.__loader__.get_data(sample.__path__[0] + "/data.txt") == b"resource"\n'
            'importlib.invalidate_caches()\ntry:\n    import sample.missing\n'
            'except ModuleNotFoundError as error:\n    assert error.name == "sample.missing"\n'
            'else:\n    raise AssertionError("missing module imported")\nprint(value)\n'
        ),
        'sample/data.txt': 'resource',
    }
    if special == 'computed':
        sources['sample/inner/__main__.py'] = 'import importlib\nname = "sample.literal"\nimportlib.import_module(name)\n'
    elif special == 'invalid':
        sources['sample/leaf.py'] = 'def bad(:\n'
    elif special == 'literal-exec':
        sources['sample/leaf.py'] = 'exec("import sample.literal")\nvalue = 42\n'
    if poison:
        member, imported = poison
        prefix = ('from sample.flags import flag\nstatus: int = flag\n'
                  if imported else 'status: int = True\n')
        sources[member] = prefix + sources[member]
    with ZipFile(path, 'w', compression=ZIP_STORED) as output:
        for name, text in sources.items():
            output.writestr(name, text)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--evidence', type=Path)
    args = parser.parse_args()
    compiler, runtime = args.compiler.resolve(), args.runtime.resolve()
    evidence = args.evidence.resolve() if args.evidence else None
    if evidence:
        evidence.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='imre-named-archive-') as directory:
        root = Path(directory)
        launcher = root / 'imre'
        shutil.copy2(compiler, launcher)
        (root / 'libimre_runtime.a').symlink_to(runtime)
        toolchain = compiler.parent / 'imre-toolchain'
        if toolchain.is_dir():
            (root / 'imre-toolchain').symlink_to(toolchain, target_is_directory=True)
        bundle = root / 'imre-stdlib.zip'
        capture = root / 'image.elf'
        env = dict(os.environ, IMREFLAGS='--opt-level 0', IMRE_CACHE_DIR=str(root / 'cache'),
                   IMRE_CAPTURE_ELF=str(capture))
        env.pop('PYTHONPATH', None)

        def run(label):
            capture.unlink(missing_ok=True)
            result = subprocess.run([launcher, '-m', 'sample.inner'], cwd=root, env=env,
                                    capture_output=True, text=True, timeout=300)
            if evidence:
                (evidence / f'{label}.out').write_text(result.stdout)
                (evidence / f'{label}.err').write_text(result.stderr)
                shutil.copy2(bundle, evidence / f'{label}.zip')
            return result

        archive(bundle)
        result = run('clean')
        assert (result.returncode, result.stdout, result.stderr) == (0, 'init\ninner\n42\n', ''), result
        assert capture.exists(), 'clean build did not produce an image'
        # An earlier namespace portion does not displace a later regular package.
        project = root / 'sample'
        project.mkdir()
        result = run('namespace')
        assert (result.returncode, result.stdout, result.stderr) == (0, 'init\ninner\n42\n', ''), result
        for member in ('sample/__init__.py', 'sample/inner/__init__.py',
                       'sample/inner/__main__.py', 'sample/leaf.py',
                       'sample/literal.py', 'sample/literal_two.py'):
            for imported in (False, True):
                archive(bundle, (member, imported))
                label = member.replace('/', '-').removesuffix('.py') + ('-imported' if imported else '-direct')
                result = run(label)
                assert result.returncode != 0 and member in result.stderr and 'expected int, got bool' in result.stderr, result
                assert not capture.exists(), f'{label} produced an image'
        for special in ('computed', 'invalid', 'literal-exec'):
            archive(bundle, special=special)
            result = run(special)
            assert result.returncode != 0 and not capture.exists(), result
            if special != 'invalid':
                assert 'unsupported' in result.stderr, result
        archive(bundle, ('sample/inner/__main__.py', False))
        # An earlier regular project package owns its subtree and must not check an SDK shadow.
        (project / '__init__.py').write_text('print("project init")\n')
        inner = project / 'inner'
        inner.mkdir()
        (inner / '__init__.py').write_text('print("project inner")\n')
        (inner / '__main__.py').write_text('print("project main")\n')
        result = run('project-shadow')
        assert (result.returncode, result.stdout, result.stderr) == (0, 'project init\nproject inner\nproject main\n', ''), result
    print('checked archive entry, dead import, namespace, precedence, resource, miss, and poison controls passed')


if __name__ == '__main__':
    main()
