import _frozen_importlib_external
import sys
import warnings
from importlib import abc, machinery, util
from importlib._abc import Loader


def current():
    assert Loader is abc.Loader is util.Loader
    assert abc.machinery is machinery
    assert issubclass(_frozen_importlib_external.NamespaceLoader, abc.InspectLoader)
    assert not hasattr(abc.MetaPathFinder, "find_spec")
    assert not hasattr(abc.PathEntryFinder, "find_spec")
    assert not hasattr(Loader, "exec_module")
    assert abc.MetaPathFinder().invalidate_caches() is None
    assert abc.PathEntryFinder().invalidate_caches() is None
    assert Loader().create_module(None) is None
    try:
        Loader().load_module("missing")
    except ImportError:
        pass
    else:
        raise AssertionError("Loader requires an exec_module hook")

    assert abc.FileLoader.__abstractmethods__ == frozenset({"get_source"})
    assert issubclass(machinery.SourceFileLoader, abc.FileLoader)
    assert issubclass(machinery.SourceFileLoader, abc.InspectLoader)
    try:
        abc.FileLoader("example", "example.py")
    except TypeError:
        pass
    else:
        raise AssertionError("constructed an abstract FileLoader")

    class File(abc.FileLoader):
        def get_source(self, fullname):
            return "value = 42"

    name, path = "imre_abc_file", "example.py"
    loader = File(name, path)
    assert loader.get_filename() == loader.get_filename(None) == loader.get_filename(name=name) == path
    try:
        loader.get_filename("other")
    except ImportError as exc:
        assert exc.name == "other"
    else:
        raise AssertionError("accepted a mismatched name")

    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        try:
            module = loader.load_module()
            assert module is sys.modules[name] and module.value == 42
            assert module.__loader__ is loader and module.__file__ == path
        finally:
            sys.modules.pop(name, None)

        class Empty(File):
            def get_source(self, fullname):
                return None

        loader = Empty(name, path)
        assert loader.get_code(name) is None
        try:
            loader.load_module()
        except ImportError:
            assert name not in sys.modules
        else:
            raise AssertionError("executed a module without a code object")

    class MemorySource(abc.SourceLoader):
        def get_filename(self, fullname):
            return "memory_plugin.py"

        def get_data(self, path):
            reads.append(path)
            return b"value = 42\r\n"

    assert abc.SourceLoader.__abstractmethods__ == frozenset({"get_data", "get_filename"})
    assert issubclass(machinery.SourceFileLoader, abc.SourceLoader)
    assert not issubclass(machinery.SourcelessFileLoader, abc.SourceLoader)
    reads = []
    loader = MemorySource()
    assert loader.get_source("memory_plugin") == "value = 42\n"
    spec = util.spec_from_loader("memory_plugin", loader)
    module = util.module_from_spec(spec)
    loader.exec_module(module)
    assert module.value == 42 and module.__file__ == spec.origin == "memory_plugin.py"
    assert module.__spec__ is spec and module.__loader__ is loader
    assert reads == ["memory_plugin.py", "memory_plugin.py"]

    class InvalidSource(MemorySource):
        def get_data(self, path):
            return b"def quote(:\n    pass\n"

    try:
        InvalidSource().get_code("memory_plugin")
    except SyntaxError as exc:
        assert (exc.filename, exc.lineno, exc.offset) == ("memory_plugin.py", 1, 11)
        assert (exc.text, exc.end_lineno, exc.end_offset) == ("def quote(:\n", 1, 12)
    else:
        raise AssertionError("compiled invalid in-memory source")
