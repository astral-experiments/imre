import importlib
import os
import sys
import tempfile
from importlib import machinery, util


class TrackingFinder:
    def __init__(self, directory):
        self.finder = machinery.FileFinder(
            directory, (machinery.SourceFileLoader, machinery.SOURCE_SUFFIXES)
        )
        self.names = []

    def find_spec(self, fullname, target=None):
        self.names.append(fullname)
        return self.finder.find_spec(fullname, target)


with tempfile.TemporaryDirectory() as directory:
    name = 'registry_probe'
    filename = os.path.join(directory, name + '.py')
    with open(filename, 'w', encoding='utf-8') as handle:
        handle.write('answer = 42\n')
    finder = TrackingFinder(directory)
    calls = []

    def path_hook(entry):
        calls.append(entry)
        return finder

    saved = sys.meta_path, sys.path, sys.path_hooks, sys.path_importer_cache
    try:
        sys.meta_path = []
        sys.path = [directory]
        sys.path_hooks = [path_hook]
        sys.path_importer_cache = {}
        assert util.find_spec(name) is None
        try:
            importlib.import_module(name)
        except ModuleNotFoundError as error:
            assert error.name == name
        else:
            raise AssertionError('uncached import ignored empty sys.meta_path')
        assert not calls
        direct = machinery.SourceFileLoader(name, filename)
        scope = {}
        exec(direct.get_code(name), scope)
        assert scope['answer'] == 42 and name not in sys.modules

        sys.meta_path.append(machinery.PathFinder)
        spec = util.find_spec(name)
        assert spec.origin == filename and spec.loader.path == filename
        assert sys.path_importer_cache[directory] is finder
        assert __import__(name).answer == 42
        assert calls == [directory] and finder.names == [name, name]

        del sys.modules[name]
        sys.path_importer_cache[directory] = None
        assert util.find_spec(name) is None
        assert finder.names == [name, name] and calls == [directory]
    finally:
        sys.meta_path, sys.path, sys.path_hooks, sys.path_importer_cache = saved
        sys.modules.pop(name, None)
print('live import registries ok')
