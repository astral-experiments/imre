"""Compare imports, docstrings and template constant identity with packed boxed providers."""

import argparse
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory


PROBE = '''\
import sys

# Both candidates have the same module name. Only the first import sees the project file.
sys.modules.pop('reprlib', None)
sys.path.insert(0, 'shadow')
import reprlib as project
sys.path.pop(0)
del sys.modules['reprlib']
import reprlib as bundled
print('shadow', project.marker(), bundled.Repr().repr([1, 2]), project is bundled)
print('docs', project.marker.__doc__, bundled.recursive_repr.__doc__, bundled.__doc__ is None)

def dormant():
    import never_loaded
print('deferred', 'never_loaded' in sys.modules)

from string.templatelib import Template
import left, right
first = left.sample(1)
second = right.sample(2)
print('slots', type(first) is Template, first.strings, second.strings,
      first.strings is left.sample(3).strings, first.strings is second.strings,
      left.sample.__code__ is right.sample.__code__)
'''


def create_project(root):
    (root / 'shadow').mkdir()
    (root / 'shadow/reprlib.py').write_text('def marker():\n    "project docs"\n    return "project"\n')
    (root / 'never_loaded.py').write_text('raise RuntimeError("eager module registration")\n')
    for name in ('left', 'right'):
        (root / f'{name}.py').write_text(f'def sample(value):\n    return t"{name}-{{value}}-end"\n')
    entry = root / 'probe.py'
    entry.write_text(PROBE)
    return entry


def provider_symbols(path):
    symbols = subprocess.check_output(['nm', '-g', '--defined-only', path])
    return {line.split()[-1] for line in symbols.splitlines() if b' imre_provider_' in line}


def check_command_reuse(root, compiler, opt_level, env):
    settings = root / 'settings.py'
    settings.write_text('value = "blue"\n')
    (root / 'stable.py').write_text('value = "stable"\n')
    env = dict(env, IMRE_CACHE_DIR=str(root / 'cache'), IMRE_CACHE_TRACE='1',
               IMRE_ENCODED_FLAGS='\x1f'.join(['--opt-level', opt_level]))
    for label, settings_action, stable_action in [
        ('first', 'miss', 'miss'), ('second', 'hit', 'hit'), ('edited', 'miss', 'hit'),
    ]:
        if label == 'edited':
            settings.write_text('value = "green"\n')
        source = f"import sys, settings, stable; print({label!r}, sys.modules['settings'].value, stable.value)"
        command = ['-B', '-c', source]
        stock = subprocess.check_output([sys.executable, *command], cwd=root, env=env)
        native = subprocess.run([compiler, *command], cwd=root, env=env, capture_output=True, check=True)
        assert native.stdout == stock, (native.stdout, stock)
        events = native.stderr.decode().splitlines()
        assert f'imre object {settings_action} settings' in events, events
        assert f'imre object {stable_action} stable' in events, events


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--opt-level', choices=('0', '3'), required=True)
    args = parser.parse_args()
    if not __debug__ or sys.version_info[:2] != (3, 14):
        parser.error('run with CPython 3.14 and assertions enabled')
    compiler = args.compiler.resolve()
    with TemporaryDirectory(prefix='imre-providers-') as temp:
        root = Path(temp)
        entry = create_project(root)
        env = {'PATH': os.environ['PATH'], 'HOME': temp, 'LC_ALL': 'C.UTF-8'}
        binary = root / 'program'
        subprocess.run([compiler, 'build', entry, '--opt-level', args.opt_level,
                        '--no-cache', '-o', binary], cwd=root, env=env, check=True)
        archives = compiler.parent / 'imre-toolchain' / 'lib' / 'imre-stdlib'
        assert provider_symbols(binary) & provider_symbols(archives / f'O{args.opt_level}/providers.a'), (
            'no provider from the installed archive was linked')

        env['IMRE_ENCODED_FLAGS'] = '\x1f'.join(['--opt-level', args.opt_level, '--no-cache'])
        for flags, command in [([], [binary]), (['-OO'], [compiler, '-B', '-OO', entry])]:
            stock = subprocess.run([sys.executable, '-B', *flags, entry], cwd=root, env=env, capture_output=True)
            native = subprocess.run(command, cwd=root, env=env, capture_output=True)
            assert stock.returncode == native.returncode == 0, (native.stderr, stock.stderr)
            assert native.stdout == stock.stdout, (flags, native.stdout, stock.stdout)
            assert native.stderr == stock.stderr == b'', (native.stderr, stock.stderr)

        check_command_reuse(root, compiler, args.opt_level, env)


if __name__ == '__main__':
    main()
