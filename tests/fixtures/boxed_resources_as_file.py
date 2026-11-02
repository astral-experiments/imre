import os
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
    try:
        with as_file(Resource('virtual-dir', ())) as root:
            assert root.is_dir()
    except NotImplementedError as exc:
        assert sys.implementation.name == 'imre'
        assert 'virtual resource directories' in str(exc)
    assert os.listdir(directory) == []
finally:
    tempfile.tempdir = previous
    os.rmdir(directory)
print('resource paths and cleanup verified')
