import os
import shutil
import sys
from pathlib import Path
import tempfile
from importlib import resources
from importlib.resources import as_file
import data


class Resource:
    name: str
    content: object

    def __init__(self, name, content):
        self.name = name
        self.content = content

    def is_dir(self):
        return isinstance(self.content, tuple)

    def iterdir(self):
        return iter(self.content)

    def read_bytes(self):
        if self.content is None:
            raise FileNotFoundError(self.name)
        return self.content


def check_directory_cleanup_veto(tree):
    blocked = None
    events = []
    denial = PermissionError('keep the resource directory')

    def veto(event, args):
        if event == 'shutil.rmtree' and args[0] == blocked:
            events.append(args)
            assert os.path.isdir(blocked)
            raise denial

    sys.addaudithook(veto)
    try:
        try:
            with as_file(tree) as copy:
                blocked = str(copy.parent)
        except PermissionError as exc:
            assert exc is denial
        else:
            raise AssertionError('resource cleanup bypassed the audit veto')
        assert events == [(blocked, None)]
        assert (copy / 'sub' / 'message.txt').read_bytes() == b'directory payload\n'
    finally:
        directory = blocked
        blocked = None
        if directory is not None and os.path.isdir(directory):
            shutil.rmtree(directory)


target = resources.files(data) / 'message.txt'
with resources.as_file(target) as local:
    assert isinstance(local, Path)
    assert local == Path(data.__file__).parent / 'message.txt'
    assert local.read_bytes() == b'hello\n'

text = Resource('utf-8.file', b'hello\n')
native = Path(__file__)
for source in (native, native.parent):
    with resources.as_file(source) as path:
        assert path is source

directory = tempfile.mkdtemp()
previous = tempfile.tempdir
tempfile.tempdir = directory
try:
    with as_file(text) as path:
        assert isinstance(path, Path)
        assert path.parent == Path(directory)
        assert path.name.endswith(text.name)
        assert path.read_text(encoding='utf-8') == 'hello\n'
    assert not path.exists()
    with as_file(text) as unlinked:
        unlinked.unlink()
    try:
        with as_file(text) as interrupted:
            raise ValueError('interrupted')
    except ValueError as exc:
        assert str(exc) == 'interrupted'
    else:
        raise AssertionError('suppressed the client exception')
    assert not interrupted.exists()

    try:
        with as_file(Resource('missing', None)):
            raise AssertionError('materialized an unreadable file')
    except FileNotFoundError:
        pass
    external = Path(data.__file__).parent / 'directory-payload.txt'
    leaf = Resource('message.txt', external.read_bytes())
    tree = Resource('virtual-dir', (Resource('sub', (leaf,)), Resource('empty', ())))
    open_fds = len(os.listdir('/proc/self/fd')) if os.path.isdir('/proc/self/fd') else None
    with as_file(tree) as root:
        assert isinstance(root, Path) and root.name == tree.name
        assert (root / 'sub' / leaf.name).read_bytes() == b'directory payload\n'
        assert (root / 'empty').is_dir()
    assert not root.parent.exists()

    check_directory_cleanup_veto(tree)

    original_mode = external.stat().st_mode
    try:
        with as_file(tree) as root:
            os.symlink(external, root / 'sub' / 'link')
            os.chmod(root / 'sub', 0)
            os.chmod(root, 0o500)
            raise ValueError('interrupted directory')
    except ValueError as exc:
        assert str(exc) == 'interrupted directory'
    else:
        raise AssertionError('suppressed the client exception')
    assert not root.parent.exists()
    assert external.stat().st_mode == original_mode

    with as_file(tree) as removed:
        shutil.rmtree(removed)
    assert not removed.parent.exists()
    partial = Resource('partial', (leaf, Resource('unreadable', None)))
    try:
        with as_file(partial):
            raise AssertionError('materialized an unreadable child')
    except FileNotFoundError as exc:
        assert exc.args == ('unreadable',)
    changing = Resource('changing', ())
    context = as_file(changing)
    changing.content = b'now a file'
    try:
        with context:
            raise RuntimeError('entered a directory that became a file')
    except AssertionError:
        pass
    if sys.implementation.name == 'imre':
        try:
            with as_file(Resource('bad', (Resource(str(external), b'invalid'),))):
                raise AssertionError('accepted a non-component child name')
        except ValueError:
            pass
        assert external.read_bytes() == leaf.content
    if open_fds is not None:
        assert len(os.listdir('/proc/self/fd')) == open_fds
    assert os.listdir(directory) == []
finally:
    tempfile.tempdir = previous
    os.rmdir(directory)
print('resource paths and cleanup verified')
