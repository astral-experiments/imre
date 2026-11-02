from importlib import import_module
from importlib.machinery import NamespaceLoader
from importlib.resources import files
from importlib.util import find_spec
from pathlib import Path
import sys

root = Path(__file__).parent
first = str(root / 'first')
second = str(root / 'second')
sys.path.insert(0, first)
initial = find_spec('acme')
assert initial.loader is None and initial.origin is None
assert list(initial.submodule_search_locations) == [str(root / 'first' / 'acme')]
package = import_module('acme')
assert find_spec('acme') is package.__spec__
assert isinstance(package.__loader__, NamespaceLoader)
assert package.__loader__ is package.__spec__.loader
assert package.__file__ is None and not package.__spec__.has_location
assert package.__path__ is package.__spec__.submodule_search_locations
sys.path.insert(1, second)
assert list(package.__path__) == [str(root / 'first' / 'acme'), str(root / 'second' / 'acme')]
assert files(package).joinpath('data.txt').read_text() == 'first'
assert files(package).joinpath('only.txt').read_text() == 'second'

if sys.platform != 'win32':
    from zipimport import zipimporter
    archive = str(root / 'portions.zip')
    portion = archive + '/acme'
    finder = zipimporter(archive)
    direct = finder.find_spec('acme')
    assert direct.loader is None and direct.submodule_search_locations == [portion]
    sys.path.insert(0, archive)
    archive_package = import_module('only_archive')
    assert archive_package.__file__ is None and archive_package.__spec__.origin is None
    assert isinstance(archive_package.__loader__, NamespaceLoader)
    assert list(archive_package.__path__) == [archive + '/only_archive']
    assert list(package.__path__) == [portion, first + '/acme', second + '/acme']
    del sys.modules['acme']
    fresh = import_module('acme')
    assert isinstance(fresh.__loader__, NamespaceLoader)
    assert list(fresh.__path__) == [portion, first + '/acme', second + '/acme']
    sys.path.remove(archive)
    sys.path.insert(2, archive)
    assert list(fresh.__path__) == [first + '/acme', second + '/acme', portion]
