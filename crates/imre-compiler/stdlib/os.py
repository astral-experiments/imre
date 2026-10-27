"""POSIX filesystem spellings for runtime-loaded modules."""
import abc
import posixpath as path
from _collections_abc import _check_methods
from posix import fspath
from types import GenericAlias


def fsdecode(filename):
    filename = fspath(filename)
    if isinstance(filename, bytes):
        return filename.decode("utf-8", "surrogateescape")
    return filename


# From CPython 3.14.5, Lib/os.py. See stdlib/docstrings/LICENSE.
class PathLike(abc.ABC):

    """Abstract base class for implementing the file system path protocol."""

    __slots__ = ()

    @abc.abstractmethod
    def __fspath__(self):
        """Return the file system path representation of the object."""
        raise NotImplementedError

    @classmethod
    def __subclasshook__(cls, subclass):
        if cls is PathLike:
            return _check_methods(subclass, '__fspath__')
        return NotImplemented

    __class_getitem__ = classmethod(GenericAlias)
