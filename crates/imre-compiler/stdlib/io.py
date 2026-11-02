# Reader and Writer from CPython 3.14.5, Lib/io.py. See stdlib/docstrings/LICENSE.
import abc
from _collections_abc import _check_methods

GenericAlias = type(list[int])


class Reader(metaclass=abc.ABCMeta):
    """Protocol for simple I/O reader instances.

    This protocol only supports blocking I/O.
    """

    __slots__ = ()

    @abc.abstractmethod
    def read(self, size=..., /):
        """Read data from the input stream and return it.

        If *size* is specified, at most *size* items (bytes/characters) will be
        read.
        """

    @classmethod
    def __subclasshook__(cls, C):
        if cls is Reader:
            return _check_methods(C, "read")
        return NotImplemented

    __class_getitem__ = classmethod(GenericAlias)


class Writer(metaclass=abc.ABCMeta):
    """Protocol for simple I/O writer instances.

    This protocol only supports blocking I/O.
    """

    __slots__ = ()

    @abc.abstractmethod
    def write(self, data, /):
        """Write *data* to the output stream and return the number of items written."""

    @classmethod
    def __subclasshook__(cls, C):
        if cls is Writer:
            return _check_methods(C, "write")
        return NotImplemented

    __class_getitem__ = classmethod(GenericAlias)
