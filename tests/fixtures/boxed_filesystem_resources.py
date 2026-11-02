from importlib import resources
import data


class Loader:
    def get_resource_reader(self, name):
        assert name == 'data'
        return self

    def files(self):
        return sentinel


root = resources.files(data)
assert root.is_dir()
assert 'nested' in [child.name for child in root.iterdir()]
target = root.joinpath('nested', 'message.txt')
assert target.is_file() and target.read_bytes() == b'hello\n'
# The source loader owns the location even if the module changes its metadata.
data.__file__ = 'wrong/__init__.py'
assert resources.read_text('data', 'nested/message.txt') == 'hello\n'
assert not (root / 'absent').is_file()
try:
    (root / 'absent').open('rb')
except FileNotFoundError:
    pass
else:
    raise AssertionError('opened a missing resource')


sentinel = object()
data.__spec__.loader = Loader()
assert resources.files(data) is sentinel
