from importlib.util import cache_from_source, module_from_spec, spec_from_file_location, spec_from_loader
import os
import sys


class Loader:
    def get_filename(self, name):
        return 'demo/__init__.py'

    def is_package(self, name):
        return True

    def create_module(self, spec):
        return None


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
