"""Abstract bases for loaders that provide package resources."""

from abc import ABC, abstractmethod


# Reader contracts from CPython 3.14.5 Lib/importlib/resources/abc.py.
# See licenses/CPython.txt.
class ResourceReader(ABC):
    """Abstract base class for loaders to provide resource reading support."""

    @abstractmethod
    def open_resource(self, resource):
        raise FileNotFoundError

    @abstractmethod
    def resource_path(self, resource):
        raise FileNotFoundError

    @abstractmethod
    def is_resource(self, path):
        raise FileNotFoundError

    @abstractmethod
    def contents(self):
        raise FileNotFoundError


class TraversableResources(ResourceReader):
    """Provide a traversable root and adapt the legacy reader methods to it."""

    @abstractmethod
    def files(self):
        """Return a Traversable object for the loaded package."""

    def open_resource(self, resource):
        return self.files().joinpath(resource).open('rb')

    def resource_path(self, resource):
        raise FileNotFoundError(resource)

    def is_resource(self, path):
        return self.files().joinpath(path).is_file()

    def contents(self):
        return (item.name for item in self.files().iterdir())
