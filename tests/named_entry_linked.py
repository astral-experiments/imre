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


def finite_archive(path, source):
    with ZipFile(path, 'w', compression=ZIP_STORED) as output:
        output.writestr('sample/__init__.py', '')
        output.writestr('sample/__main__.py', source)
        output.writestr('sample/alias.py', 'status: int = True\n')


def saved_application(compiler, runtime, root, clang=None):
    """Check selected named application imports on the entry and a separate PYTHONPATH root."""
    cwd, search = root / 'saved-cwd', root / 'saved-search'
    cwd.mkdir()
    (search / 'named_package').mkdir(parents=True)
    (search / 'named_package' / '__init__.py').write_text('')
    (search / 'named_package' / '__main__.py').write_text(
        "import keyword, typing, abc, enum\nprint(keyword.iskeyword('for'))\n")
    dependency = search / 'named_dependency.py'
    dependency.write_text('status: int = 42\n')
    env = dict(os.environ, PYTHONPATH=str(search.resolve()), PYTHONDONTWRITEBYTECODE='1',
               PYTHONNOUSERSITE='1')
    command = [str(compiler), 'build', '-m', 'named_package', '--quiet', '--no-cache',
               '--opt-level', '0', '--runtime', str(runtime)]
    if clang:
        command += ['--clang', str(clang)]

    def build(image, environment):
        return subprocess.run(command + ['-o', str(image)], cwd=cwd, env=environment,
                              capture_output=True, text=True, timeout=180)

    def rejects(path, environment, image):
        poison = build(image, environment)
        assert poison.returncode != 0 and not image.exists(), poison
        assert f'{path}:1:15:' in poison.stderr and 'expected int, got bool' in poison.stderr, poison

    mixed_image = root / 'mixed-provider-image'
    mixed = build(mixed_image, env)
    assert mixed.returncode == 0 and mixed_image.exists(), mixed
    result = subprocess.run([mixed_image], cwd=cwd, env=env, capture_output=True, text=True,
                            timeout=20)
    assert (result.returncode, result.stdout, result.stderr) == (0, 'True\n', ''), result
    changed_typing = search / 'typing.py'
    changed_typing.write_text('status: int = True\n')
    rejects(changed_typing, env, root / 'changed-typing-rejected')
    changed_typing.unlink()
    (search / 'named_package' / '__main__.py').write_text(
        'import named_dependency\nprint(named_dependency.status)\n')

    image = root / 'saved-image'
    clean = build(image, env)
    assert clean.returncode == 0 and image.exists(), clean
    result = subprocess.run([image], cwd=cwd, env=env, capture_output=True, text=True,
                            timeout=20)
    assert (result.returncode, result.stdout, result.stderr) == (0, '42\n', ''), result

    dependency.write_text('status: int = True\n')
    rejects(dependency, env, root / 'same-root-rejected')
    dependency.unlink()
    cross_search = root / 'saved-cross-search'
    cross_search.mkdir()
    cross_dependency = cross_search / 'named_dependency.py'
    cross_dependency.write_text('status: int = True\n')
    cross_env = dict(env, PYTHONPATH=os.pathsep.join((str(search.resolve()), str(cross_search.resolve()))))
    rejects(cross_dependency, cross_env, root / 'cross-root-rejected')

    clean_search = root / 'saved-clean-search'
    clean_search.mkdir()
    (clean_search / 'named_dependency.py').write_text('status: int = 42\n')
    shadow_env = dict(env, PYTHONPATH=os.pathsep.join(
        (str(search.resolve()), str(clean_search.resolve()), str(cross_search.resolve()))))
    shadow_image = root / 'shadow-image'
    shadow = build(shadow_image, shadow_env)
    assert shadow.returncode == 0 and shadow_image.exists(), shadow
    result = subprocess.run([shadow_image], cwd=cwd, env=shadow_env, capture_output=True,
                            text=True, timeout=20)
    assert (result.returncode, result.stdout, result.stderr) == (0, '42\n', ''), result

    marker = root / 'shadow-executed'
    cross_dependency.write_text(f"open({str(marker)!r}, 'w').write('executed')\nstatus: int = True\n")
    (search / 'named_package' / '__main__.py').write_text(
        f"import sys\nsys.path.insert(0, {str(cross_search)!r})\nimport named_dependency\n")
    changed_image = root / 'changed-path-image'
    changed = build(changed_image, shadow_env)
    if changed.returncode:
        assert changed.stderr and not changed_image.exists(), changed
    else:
        result = subprocess.run([changed_image], cwd=cwd, env=shadow_env, capture_output=True,
                                text=True, timeout=20)
        assert result.returncode != 0 and 'no compiled Python body' in result.stderr, result
    assert not marker.exists()



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

        def run(label, operand='sample.inner'):
            capture.unlink(missing_ok=True)
            result = subprocess.run([launcher, '-m', operand], cwd=root, env=env,
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
        shadow = (
            'import importlib\n'
            'class C:\n'
            '    def import_module(self, name):\n'
            "        print('custom', name)\n"
            'def f(importlib, name):\n'
            '    importlib.import_module(name)\n'
            "f(C(), 'hello')\n"
        )
        shadows = (
            ('custom-shadow', shadow),
            ('local-shadow', 'import importlib\nclass C:\n    def import_module(self, name): print("custom", name)\ndef f(name):\n    importlib = C()\n    importlib.import_module(name)\nf("hello")\n'),
            ('closure-shadow', 'import importlib\nclass C:\n    def import_module(self, name): print("custom", name)\ndef outer():\n    importlib = C()\n    def inner(name): importlib.import_module(name)\n    inner("hello")\nouter()\n'),
            ('rebind-shadow', 'import importlib\nclass C:\n    def import_module(self, name): print("custom", name)\nimportlib = C()\nimportlib.import_module("hello")\n'),
            ('builtin-shadow', 'def __import__(name): print("custom", name)\n__import__("hello")\n'),
            ('exec-shadow', "def exec(source):\n    print('custom', source)\nexec('hello')\n"),
        )
        for label, source in shadows:
            finite_archive(bundle, source)
            result = run(label, 'sample')
            assert (result.returncode, result.stdout, result.stderr) == (0, 'custom hello\n', ''), result
            assert capture.exists(), f'{label} did not produce an image'
        for label, source in (
            ('real-computed', 'import importlib\nname = "sample.alias"\nimportlib.import_module(name)\n'),
            ('real-alias', 'from importlib import import_module as load\nload("sample.alias")\n'),
            ('forward-alias', 'def f(): load("sample.alias")\nfrom importlib import import_module as load\nf()\n'),
            ('forward-computed', 'def f(name): load(name)\nfrom importlib import import_module as load\nf("sample.alias")\n'),
            ('real-builtin', '__import__("sample.alias")\n'),
            ('builtin-fromlist', '__import__("sample", fromlist=("alias",))\n'),
            ('importer-expansion', 'import importlib\noptions = {}\nimportlib.import_module("sample.alias", **options)\n'),
            ('real-exec', "exec(\"print('should not run')\")\n"),
            ('importer-transfer', 'import importlib\ndef f(fn, name): fn(name)\nf(importlib.import_module, "sample.alias")\n'),
            ('importer-alias-transfer', 'from importlib import import_module as load\ndef f(fn, name): fn(name)\nf(load, "sample.alias")\n'),
            ('importer-assignment', 'from importlib import import_module as load\nsaved = load\n'),
            ('importer-return', 'from importlib import import_module as load\ndef take(): return load\ntake()\n'),
            ('importer-list', 'from importlib import import_module as load\nhandlers = [load]\n'),
            ('executor-transfer', 'def f(fn): pass\nf(exec)\n'),
        ):
            finite_archive(bundle, source)
            result = run(label, 'sample')
            assert result.returncode != 0 and not capture.exists(), result
            if label not in ('real-alias', 'forward-alias', 'real-builtin'):
                assert 'unsupported' in result.stderr, result
            else:
                assert 'sample/alias.py' in result.stderr and 'expected int, got bool' in result.stderr, result
        archive(bundle)
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
        saved_application(launcher, runtime, root,
                          toolchain / 'bin' / 'clang' if toolchain.is_dir() else None)
    print('checked archive entry, dead import, namespace, precedence, resource, miss, and poison controls passed')


if __name__ == '__main__':
    main()
