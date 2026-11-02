"""Abstract base classes for the native import system."""

import abc
import _frozen_importlib_external as _bootstrap_external
from ._abc import Loader
from . import machinery

__all__ = [
    "Loader", "MetaPathFinder", "PathEntryFinder", "ResourceLoader",
    "InspectLoader", "ExecutionLoader", "FileLoader",
]


class MetaPathFinder(metaclass=abc.ABCMeta):
    def invalidate_caches(self):
        pass


class PathEntryFinder(metaclass=abc.ABCMeta):
    def invalidate_caches(self):
        pass


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


InspectLoader.register(_bootstrap_external.NamespaceLoader)


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


FileLoader.register(machinery.SourceFileLoader)
