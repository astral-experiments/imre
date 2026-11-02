"""POSIX filesystem spellings for runtime-loaded modules."""
import abc
import posixpath as path
from _collections_abc import _check_methods
from _collections_abc import MutableMapping as _MutableMapping
from posix import access, environ as _environ, fspath, getcwd, getcwdb, listdir, mkdir, putenv, read, terminal_size, unsetenv, waitpid, write, WNOHANG
from types import GenericAlias


curdir = path.curdir


# From CPython 3.14.5, Lib/os.py. See stdlib/docstrings/LICENSE.
def makedirs(name, mode=0o777, exist_ok=False):
    """makedirs(name [, mode=0o777][, exist_ok=False])

    Super-mkdir; create a leaf directory and all intermediate ones.  Works like
    mkdir, except that any intermediate path segment (not just the rightmost)
    will be created if it does not exist. If the target directory already
    exists, raise an OSError if exist_ok is False. Otherwise no exception is
    raised.  This is recursive.

    """
    head, tail = path.split(name)
    if not tail:
        head, tail = path.split(head)
    if head and tail and not path.exists(head):
        try:
            makedirs(head, exist_ok=exist_ok)
        except FileExistsError:
            # Defeats race condition when another thread created the path
            pass
        cdir = curdir
        if isinstance(tail, bytes):
            cdir = bytes(curdir, 'ASCII')
        if tail == cdir:           # xxx/newdir/. exists if xxx/newdir exists
            return
    try:
        mkdir(name, mode)
    except OSError:
        # Cannot rely on checking for EEXIST, since the operating system
        # could give priority to other errors like EACCES or EROFS
        if not exist_ok or not path.isdir(name):
            raise


def fsencode(filename):
    filename = fspath(filename)
    if isinstance(filename, str):
        return filename.encode("utf-8", "surrogateescape")
    return filename


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

    def copy(self):
        return dict(self)


environ = _Environ(_environ)


def getenv(key, default=None):
    return environ.get(key, default)
