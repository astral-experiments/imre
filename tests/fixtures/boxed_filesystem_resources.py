from io import BytesIO
from importlib import resources
from importlib.resources.abc import ResourceReader, Traversable, TraversableResources, TraversalError
import data.child


class Loader:
    def __init__(self, name, root):
        self.name = name
        self.root = root

    def get_resource_reader(self, name):
        assert name == self.name
        return self

    def files(self):
        return self.root


class Reader(TraversableResources):
    def files(self):
        return root


class LegacyReader:
    def get_resource_reader(self, name):
        assert name == 'data'
        return self

    def contents(self):
        return ('message.txt', 'dir')

    def is_resource(self, name):
        return name == 'message.txt'

    def open_resource(self, name):
        if name != 'message.txt':
            raise FileNotFoundError(name)
        return BytesIO(b'caf\xc3\xa9\n')


root = resources.files(data)
assert isinstance(root, Traversable)
assert not isinstance(object(), Traversable)
assert Traversable.joinpath(root, 'nested/message.txt').read_bytes() == b'hello\n'
try:
    Traversable.joinpath(root, 'absent')
except TraversalError:
    pass
else:
    raise AssertionError('default Traversable.joinpath found a missing child')
assert root.is_dir()
assert 'nested' in [child.name for child in root.iterdir()]
target = root.joinpath('nested', 'message.txt')
assert target.is_file() and target.read_bytes() == b'hello\n'
# The source loader owns the location even if the module changes its metadata.
data.__file__ = 'wrong/__init__.py'
assert resources.read_text('data', 'nested/message.txt') == 'hello\n'
assert resources.read_binary(data, 'nested', 'message.txt') == b'hello\n'
assert resources.read_binary('data', 'nested/message.txt') == b'hello\n'
assert not (root / 'absent').is_file()
try:
    (root / 'absent').open('rb')
except FileNotFoundError:
    pass
else:
    raise AssertionError('opened a missing resource')


for base in (ResourceReader, TraversableResources):
    try:
        base()
    except TypeError:
        pass
    else:
        raise AssertionError('constructed an abstract reader')


reader = Reader()
assert isinstance(reader, ResourceReader)
assert reader.is_resource('nested/message.txt')
assert 'nested' in list(reader.contents())
with reader.open_resource('nested/message.txt') as stream:
    assert stream.read() == b'hello\n'
try:
    reader.resource_path('nested/message.txt')
except FileNotFoundError as error:
    assert error.args == ('nested/message.txt',)
else:
    raise AssertionError('reader unexpectedly provided a filesystem path')


sentinel = object()
data.__spec__.loader = Loader('data', sentinel)
child_root = object()
data.child.__spec__.loader = Loader('data.child', child_root)
assert resources.files(data) is sentinel
assert resources.files('data.child') is child_root
assert data.child.call(resources.files) is child_root
assert data.child.call(data.wrapper) is child_root
assert data.child.call(data.regular) is sentinel


data.__spec__.loader = LegacyReader()
root = resources.files(data)
assert root.name == 'data'
assert [p.name for p in root.iterdir()] == ['message.txt', 'dir']
assert (root / 'dir').is_dir()
message = root / 'message.txt'
assert message.is_file() and not list(message.iterdir())
assert message.read_bytes() == b'caf\xc3\xa9\n'
assert resources.read_binary(data, 'message.txt') == b'caf\xc3\xa9\n'
assert message.read_text(encoding='utf-8') == 'café\n'
try:
    (root / 'absent').read_bytes()
except FileNotFoundError:
    pass
else:
    raise AssertionError('suppressed a reader error')

data.__spec__.loader = None
orphan = resources.files(data) / 'missing' / 'child'
assert orphan.name == 'child' and not orphan.is_file() and not orphan.is_dir()
try:
    orphan.read_bytes()
except FileNotFoundError:
    pass
else:
    raise AssertionError('opened an orphan resource')
