"""Abstract base classes for the native import system."""

import abc
from . import _bootstrap_external
from ._abc import Loader
from . import machinery

try:
    import _frozen_importlib
except ImportError as exc:
    if exc.name != '_frozen_importlib':
        raise
    _frozen_importlib = None
try:
    import _frozen_importlib_external
except ImportError:
    _frozen_importlib_external = _bootstrap_external

__all__ = [
    "Loader", "MetaPathFinder", "PathEntryFinder", "ResourceLoader",
    "InspectLoader", "ExecutionLoader", "FileLoader", "SourceLoader",
]


def _register(abstract_cls, *classes):
    for cls in classes:
        abstract_cls.register(cls)
        if _frozen_importlib is not None:
            try:
                frozen_cls = getattr(_frozen_importlib, cls.__name__)
            except AttributeError:
                frozen_cls = getattr(_frozen_importlib_external, cls.__name__)
            abstract_cls.register(frozen_cls)


class MetaPathFinder(metaclass=abc.ABCMeta):
    def invalidate_caches(self):
        pass


class PathEntryFinder(metaclass=abc.ABCMeta):
    def invalidate_caches(self):
        pass


_register(MetaPathFinder, machinery.BuiltinImporter, machinery.PathFinder)
_register(PathEntryFinder, machinery.FileFinder)


class ResourceLoader(Loader):
    @abc.abstractmethod
    def get_data(self, path):
        raise OSError


class InspectLoader(Loader):
    def is_package(self, fullname):
        raise ImportError

    def get_code(self, fullname):
        source = self.get_source(fullname)
        if source is None:
            return None
        return self.source_to_code(source)

    @abc.abstractmethod
    def get_source(self, fullname):
        raise ImportError

    @staticmethod
    def source_to_code(data, path="<string>"):
        return compile(data, path, "exec")

    exec_module = _bootstrap_external._LoaderBasics.exec_module
    load_module = _bootstrap_external._LoaderBasics.load_module


_register(InspectLoader, machinery.BuiltinImporter, machinery.NamespaceLoader)


class ExecutionLoader(InspectLoader):
    @abc.abstractmethod
    def get_filename(self, fullname):
        raise ImportError

    def get_code(self, fullname):
        source = self.get_source(fullname)
        if source is None:
            return None
        try:
            path = self.get_filename(fullname)
        except ImportError:
            return self.source_to_code(source)
        else:
            return self.source_to_code(source, path)


class FileLoader(_bootstrap_external.FileLoader, ResourceLoader, ExecutionLoader):
    pass


_register(FileLoader, machinery.SourceFileLoader, machinery.SourcelessFileLoader)


class SourceLoader(_bootstrap_external.SourceLoader, ResourceLoader, ExecutionLoader):

    """Abstract base class for loading source code (and optionally any
    corresponding bytecode).

    To support loading from source code, the abstractmethods inherited from
    ResourceLoader and ExecutionLoader need to be implemented. To also support
    loading from bytecode, the optional methods specified directly by this ABC
    is required.

    Inherited abstractmethods not implemented in this ABC:

        * ResourceLoader.get_data
        * ExecutionLoader.get_filename

    """

    def path_mtime(self, path):
        """Return the (int) modification time for the path (str)."""
        import warnings
        warnings.warn('SourceLoader.path_mtime is deprecated in favour of '
                      'SourceLoader.path_stats().',
                      DeprecationWarning, stacklevel=2)
        if self.path_stats.__func__ is SourceLoader.path_stats:
            raise OSError
        return int(self.path_stats(path)['mtime'])

    def path_stats(self, path):
        """Return a metadata dict for the source pointed to by the path (str).
        Possible keys:
        - 'mtime' (mandatory) is the numeric timestamp of last source
          code modification;
        - 'size' (optional) is the size in bytes of the source code.
        """
        if self.path_mtime.__func__ is SourceLoader.path_mtime:
            raise OSError
        return {'mtime': self.path_mtime(path)}

    def set_data(self, path, data):
        """Write the bytes to the path (if possible).

        Accepts a str path and data as bytes.

        Any needed intermediary directories are to be created. If for some
        reason the file cannot be written because of permissions, fail
        silently.
        """

_register(SourceLoader, machinery.SourceFileLoader)
