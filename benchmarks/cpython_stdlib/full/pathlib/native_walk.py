"""Walk complete upstream filesystem fixtures with retained native cursors."""

from collections.abc import Iterator
import hashlib
import inspect
import os
import subprocess
import sys

from native_traversal import Bridge, HERE, MODULE, PREFIX, READ, facade, registered_body
from suite_evidence import roles
from suite_native import native_method

WALK = PREFIX + 'PathWalkTest.'


def fixture(case):
    fields = ({'root': case.root} if hasattr(case, 'root') else
              {'walk_path': case.walk_path, 'sub2_path': case.sub2_path, 'link_path': case.link_path})
    can_symlink = (case.ground if hasattr(case, 'ground') else case).can_symlink
    declarations = ''.join(f'    {name}: Path\n' for name in fields)
    initialize = ''.join(f'        self.{name} = Path({os.fspath(value)!r})\n' for name, value in fields.items())
    return f'''from pathlib import Path
class Ground:
    can_symlink: bool
    def __init__(self) -> None:
        self.can_symlink = {can_symlink!r}
class NativeCase:
    ground: Ground
{declarations}    def __init__(self) -> None:
        self.ground = Ground()
{initialize}    def assertEqual(self, first: list[str], second: list[str]) -> None:
        if len(first) != len(second):
            raise AssertionError("different lengths")
        for i in range(len(first)):
            if first[i] != second[i]:
                raise AssertionError("different names")
    def assertTrue(self, value: bool) -> None:
        if not value:
            raise AssertionError("expected true")
    def assertFalse(self, value: bool) -> None:
        if value:
            raise AssertionError("expected false")
    def assertIn(self, name: str, names: list[str]) -> None:
        if name not in names:
            raise AssertionError("missing name")
    def fail(self, message: str) -> None:
        raise AssertionError(message)
'''


def native_walk(tools, directory, case, original):
    if hasattr(case, 'root'):
        return native_method(tools, directory, MODULE.with_name('test_read.py'), 'ReadTestBase',
                             case.id(), fixture(case), operation=('imre', 'Path.walk bottom up'))(case, original)
    body = registered_body(case, original, 'PathWalkTest', 'symlinks')
    entry = fixture(case).encode() + body + f'\nNativeCase().{case._testMethodName}()\n'.encode()
    result = subprocess.run([tools.build(entry, directory)], capture_output=True, timeout=60)
    if result.returncode or result.stderr or result.stdout:
        raise AssertionError((case.id(), result.returncode, result.stderr, result.stdout))
    return {'roles': roles(body='imre', assertions='imre', operation='imre', fixture='mixed'),
            'evidence': {'operation': 'Path.walk symlink classification and traversal',
                         'fixture': 'original needs_symlinks registry and setUp; native test instance',
                         'method_sha256': hashlib.sha256(body).hexdigest(), 'opt_level': tools.level}}


def host_walk(binary, directory, case, original):
    bridge = Bridge(binary, directory, os.getcwd())
    cls = facade(bridge)
    host = set()

    class Names(list):
        def sort(self, *args, **kwargs):
            host.add('caller sorts a yielded name list in CPython')
            return super().sort(*args, **kwargs)

        def remove(self, name):
            host.add('caller removes a yielded name in CPython; edits applied to retained native dirnames')
            return super().remove(name)

    class Rows(Iterator):
        def __init__(self, handle):
            self.handle = handle
            self.pending = None

        def __iter__(self):
            if not bridge.call('walk_iter', self.handle):
                raise AssertionError('native walk cursor lost identity')
            return self

        def __next__(self):
            if self.pending is not None:
                index, original_dirs, current = self.pending
                if original_dirs != current:
                    bridge.call('walk_update_dirs', index, len(current), *current)
            number, index, dirs, files = bridge.call('walk_next', self.handle)
            current = Names(dirs)
            self.pending = index, dirs, current
            return cls.wrap(number), current, Names(files)

    def walk(path, top_down=True, on_error=None, follow_symlinks=False):
        if on_error is not None:
            raise ValueError('walk fixture requires native on_error callback transport')
        return Rows(bridge.call('walk', path.number, str(top_down).lower(), str(follow_symlinks).lower()))

    cls.walk = walk
    patches = {'root': cls(os.fspath(case.root))} if hasattr(case, 'root') else {'cls': cls}
    saved = {k: (k in vars(case), getattr(case, k)) for k in patches}
    previous = sys.getprofile()
    def profile(frame, event, arg):
        if event == 'call' and frame.f_globals.get('__name__', '').split('.')[0] == 'pathlib':
            host.add(frame.f_globals['__name__'] + '.' + frame.f_code.co_qualname)
    try:
        for k, v in patches.items():
            setattr(case, k, v)
        sys.setprofile(profile)
        original()
    finally:
        sys.setprofile(previous)
        for k, (existed, v) in saved.items():
            if existed:
                setattr(case, k, v)
            else:
                delattr(case, k)
        bridge.close()
    if not bridge.calls['walk'] or not bridge.calls['walk_next']:
        raise AssertionError('original method did not drive native walk')
    return {'roles': roles(body='cpython', assertions='cpython', operation='mixed' if host else 'imre',
                           fixture='cpython'),
            'evidence': {'operation': 'Path.walk', 'fixture': 'original CPython hooks; Imre holds paths and cursors',
                         'native_calls': dict(sorted(bridge.calls.items())), 'host_product_calls': sorted(host),
                         'method_source_sha256': hashlib.sha256(inspect.getsource(original).encode()).hexdigest()}}


def walk_adapters(tools, work):
    binary = tools.build((HERE / 'traversal_driver.py').read_bytes(), work)
    native = {WALK + 'test_walk_follow_symlinks', WALK + 'test_walk_symlink_location', READ + 'test_walk_bottom_up'}
    host = {WALK + 'test_walk_many_open_files', WALK + 'test_walk_above_recursion_limit',
            READ + 'test_walk_top_down', READ + 'test_walk_prune'}
    def run(case, original):
        directory = work / case.id()
        directory.mkdir()
        if case.id() in native:
            return native_walk(tools, directory, case, original)
        return host_walk(binary, directory, case, original)
    return dict.fromkeys(native | host, run)
