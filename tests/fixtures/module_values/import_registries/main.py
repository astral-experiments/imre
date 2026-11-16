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
        # Import hooks such as Ansible's delegate filesystem entries to the installed FileFinder.
        file_hook, = [hook for hook in sys.path_hooks if 'FileFinder' in repr(hook)]
        disk_finder = file_hook(directory)
        assert type(disk_finder) is machinery.FileFinder
        assert disk_finder.find_spec(name).origin == filename
        sys.path_hooks = [hook for hook in sys.path_hooks if hook is not file_hook]
        sys.path_importer_cache = {}
        assert machinery.PathFinder.find_spec(name, [directory]) is None

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
        # The direct loader must work even when ordinary imports cannot find its helpers.
        operator_module = sys.modules.pop('operator', None)
        os_module = sys.modules.pop('os', None)
        try:
            exec(direct.get_code(name), scope)
            assert direct.source_to_code(b'pass', b'probe.py').co_filename == 'probe.py'
        finally:
            if os_module is not None:
                sys.modules['os'] = os_module
            if operator_module is not None:
                sys.modules['operator'] = operator_module
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
# An application namespace portion precedes the standard library on sys.path.
with tempfile.TemporaryDirectory() as directory:
    saved_module = sys.modules.pop('keyword', None)
    os.mkdir(os.path.join(directory, 'keyword'))
    try:
        regular = util.find_spec('keyword')
        sys.path.insert(0, directory)
        try:
            portion = machinery.PathFinder.find_spec('keyword', [directory])
            assert portion.loader is None
            assert list(portion.submodule_search_locations) == [os.path.join(directory, 'keyword')]
            finder = machinery.PathFinder
            original_find_spec = finder.__dict__['find_spec']

            def choose_namespace(cls, name, path=None, target=None):
                assert name == 'keyword'
                return portion

            finder.find_spec = classmethod(choose_namespace)
            try:
                assert util.find_spec('keyword') is portion
            finally:
                finder.find_spec = original_find_spec

            actual = util.find_spec('keyword')
            assert actual.origin == regular.origin and type(actual.loader) is type(regular.loader)
            import keyword
            assert keyword.iskeyword('for')
        finally:
            sys.path.remove(directory)
    finally:
        sys.modules.pop('keyword', None)
        if saved_module is not None:
            sys.modules['keyword'] = saved_module
print('live import registries ok')
