import importlib.abc
from importlib.util import cache_from_source, module_from_spec, resolve_name, spec_from_file_location, spec_from_loader
import os
import sys
import warnings


class Loader:
    def get_filename(self, name):
        return 'demo/__init__.py'

    def is_package(self, name):
        return True

    def create_module(self, spec):
        return None


assert resolve_name('..util', 'importlib.metadata') == 'importlib.util'
assert resolve_name('..', 'importlib.metadata') == 'importlib'

loader = Loader()
tag = sys.implementation.cache_tag
try:
    sys.implementation.cache_tag = 'fixture'
    spec = spec_from_loader('demo', loader)
    assert spec.loader is loader
    assert spec.origin == loader.get_filename('demo')
    assert spec.submodule_search_locations == [os.path.dirname(spec.origin)]
    assert spec.cached == cache_from_source(spec.origin)
    module = module_from_spec(spec)
    assert module.__spec__ is spec
    assert module.__loader__ is loader
    assert module.__path__ is spec.submodule_search_locations

    locations = ['other']
    located = spec_from_file_location('other', 'other.py', loader=loader,
                                      submodule_search_locations=locations)
    assert located.origin == os.path.abspath('other.py')
    assert located.submodule_search_locations is locations
    sys.implementation.cache_tag = None
    assert located.cached is None
    assert spec.cached is not None
finally:
    sys.implementation.cache_tag = tag

# A registry origin belongs to the live spec even when the module has another filename.
spec = spec_from_loader('demo.plugin', loader, origin='registry:v1', is_package=False)
module = module_from_spec(spec)
module.__file__ = 'stale.py'
assert repr(module) == "<module 'demo.plugin' (registry:v1)>"
spec.origin = 'registry:v2'
assert str([module]) == "[<module 'demo.plugin' (registry:v2)>]"


# Legacy load_module consults the selected core's spec factory on every load.
class MemoryLoader(importlib.abc.Loader):
    def exec_module(self, module):
        assert module is sys.modules[module.__name__]
        module.generation = module.__spec__.loader_state


name = 'spec_factory_plugin'
loader = MemoryLoader()
factory = importlib._bootstrap.spec_from_loader
specs = []


def with_generation(name, loader):
    spec = factory(name, loader)
    spec.loader_state = len(specs) + 1
    specs.append(spec)
    return spec


try:
    importlib._bootstrap.spec_from_loader = with_generation
    with warnings.catch_warnings():
        warnings.simplefilter('ignore', DeprecationWarning)
        first = loader.load_module(name)
        assert first.generation == 1 and first.__spec__ is specs[0]
        second = loader.load_module(name)
        assert second is first and second.generation == 2 and second.__spec__ is specs[1]
finally:
    importlib._bootstrap.spec_from_loader = factory
    sys.modules.pop(name, None)
