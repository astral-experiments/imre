# Public I/O ABCs and protocols from CPython 3.14.5, Lib/io.py. See stdlib/docstrings/LICENSE.
import _io
import abc
from _io import (BufferedRWPair, BufferedReader, BufferedWriter, BytesIO, DEFAULT_BUFFER_SIZE, FileIO,
                 StringIO, TextIOWrapper, UnsupportedOperation, open, open_code, text_encoding)
from _collections_abc import _check_methods

class IOBase(_io._IOBase, metaclass=abc.ABCMeta):
    __doc__ = _io._IOBase.__doc__

class RawIOBase(_io._RawIOBase, IOBase):
    __doc__ = _io._RawIOBase.__doc__

class BufferedIOBase(_io._BufferedIOBase, IOBase):
    __doc__ = _io._BufferedIOBase.__doc__

class TextIOBase(_io._TextIOBase, IOBase):
    __doc__ = _io._TextIOBase.__doc__

RawIOBase.register(FileIO)

for klass in (BytesIO, BufferedReader, BufferedWriter, BufferedRWPair, _buffered_random):
    BufferedIOBase.register(klass)

for klass in (StringIO, TextIOWrapper):
    TextIOBase.register(klass)
del klass, _buffered_random

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
