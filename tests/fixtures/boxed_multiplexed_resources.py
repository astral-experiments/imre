from importlib import readers as compatibility
from importlib.resources import readers
from importlib.readers import MultiplexedPath
from pathlib import Path
import warnings


class BrokenRepr:
    def __repr__(self):
        raise RuntimeError('repr called')


assert MultiplexedPath is readers.MultiplexedPath is compatibility.MultiplexedPath
assert readers is not compatibility
assert compatibility.NamespaceReader is readers.NamespaceReader

first = Path(__file__).parent / 'first'
second = first.parent / 'second'
merged = readers.MultiplexedPath(first, first, second)
assert {path.name for path in merged.iterdir()} == {'same.txt', 'shared', 'mixed'}
assert merged.joinpath('same.txt').read_text() == 'first'
assert readers.MultiplexedPath(second, first).joinpath('same.txt').read_text() == 'second'
assert merged.joinpath('mixed') == first / 'mixed'
assert readers.MultiplexedPath(second, first).joinpath('mixed') == second / 'mixed'
subdir = merged.joinpath('shared')
assert isinstance(subdir, readers.MultiplexedPath)
assert sorted(child.name for child in subdir.iterdir()) == ['one', 'two']
assert not merged.joinpath('absent', 'child').exists()
assert readers.MultiplexedPath._follow(iter(())) is None
assert readers.NamespaceReader._resolve(str(first)) == first
assert readers.NamespaceReader._resolve(str(first / 'absent')) is None
try:
    readers.MultiplexedPath._follow([BrokenRepr(), BrokenRepr()])
except RuntimeError as error:
    assert str(error) == 'repr called'
else:
    raise AssertionError('group check suppressed a repr error')

try:
    readers.MultiplexedPath(first / 'same.txt')
except NotADirectoryError:
    pass
else:
    raise AssertionError('file accepted as a directory')
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter('always')
    assert readers.MultiplexedPath(str(first)).name == first.name
assert len(caught) == 1 and caught[0].category is DeprecationWarning
