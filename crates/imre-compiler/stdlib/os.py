"""POSIX filesystem spellings for runtime-loaded modules."""
import abc
import posixpath as path
from _collections_abc import _check_methods
from _collections_abc import MutableMapping as _MutableMapping
from posix import environ as _environ, fspath, putenv, unsetenv
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


def _encode_env(value):
    if not isinstance(value, str):
        raise TypeError("str expected, not %s" % type(value).__name__)
    return value.encode("utf-8", "surrogateescape")


class _Environ(_MutableMapping):
    def __init__(self, data):
        self._data = data

    def __getitem__(self, key):
        try:
            value = self._data[_encode_env(key)]
        except KeyError:
            raise KeyError(key) from None
        return value.decode("utf-8", "surrogateescape")

    def __setitem__(self, key, value):
        key = _encode_env(key)
        value = _encode_env(value)
        putenv(key, value)
        self._data[key] = value

    def __delitem__(self, key):
        encoded = _encode_env(key)
        unsetenv(encoded)
        try:
            del self._data[encoded]
        except KeyError:
            raise KeyError(key) from None

    def __iter__(self):
        for key in list(self._data):
            yield key.decode("utf-8", "surrogateescape")

    def __len__(self):
        return len(self._data)


environ = _Environ(_environ)


def getenv(key, default=None):
    return environ.get(key, default)
