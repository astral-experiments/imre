"""Read gzip-compressed files using Imre's streaming decoder."""

import io
import operator
import os
import _imre_gzip
from _imre_gzip import BadGzipFile


class GzipFile(io.BufferedIOBase):
    """A binary reader for one or more gzip members in a filesystem file."""

    mode = "rb"
    _reader = None

    def __init__(self, filename=None, mode=None, compresslevel=9, fileobj=None, mtime=None):
        if mode in ("w", "wb", "a", "ab", "x", "xb"):
            raise io.UnsupportedOperation("gzip writing is not supported")
        if mode not in (None, "r", "rb"):
            raise ValueError(f"Invalid mode: {mode!r}")
        if fileobj is not None:
            raise io.UnsupportedOperation("gzip reading from a file object is not supported")
        path = os.fspath(filename)
        reader = _imre_gzip.open(path)
        if self._reader is not None:
            self.close()
        self.name = path
        self._reader = reader

    @property
    def closed(self):
        return self._reader is None

    def _open_reader(self):
        if self.closed:
            raise ValueError("I/O operation on closed file")
        return self._reader

    def readable(self):
        return True

    def read(self, size=-1):
        return _imre_gzip.read(self._open_reader(), -1 if size is None else size)

    def read1(self, size=-1):
        reader = self._open_reader()
        size = operator.index(size)
        return _imre_gzip.read(reader, io.DEFAULT_BUFFER_SIZE if size < 0 else size)

    def readline(self, size=-1):
        reader = self._open_reader()
        size = -1 if size is None else operator.index(size)
        if size < 0:
            return _imre_gzip.readline(reader)
        return super().readline(size)

    def fileno(self):
        return _imre_gzip.fileno(self._open_reader())

    def flush(self, zlib_mode=2):
        self._open_reader()

    def __enter__(self):
        self._open_reader()
        return self

    def close(self):
        reader = self._reader
        if reader is not None:
            self._reader = None
            _imre_gzip.close(reader)


def open(filename, mode="rb", compresslevel=9, encoding=None, errors=None, newline=None):
    """Open a gzip-compressed path in binary or text read mode."""
    if "t" in mode:
        if "b" in mode:
            raise ValueError(f"Invalid mode: {mode!r}")
    elif encoding is not None or errors is not None or newline is not None:
        raise ValueError("encoding, errors and newline are only supported in text mode")
    if not isinstance(filename, (str, bytes, os.PathLike)):
        if hasattr(filename, "read") or hasattr(filename, "write"):
            raise io.UnsupportedOperation("gzip reading from a file object is not supported")
        raise TypeError("filename must be a str or bytes object, or a file")
    stream = GzipFile(filename, mode.replace("t", ""), compresslevel)
    if "t" not in mode:
        return stream
    try:
        return io.TextIOWrapper(stream, encoding, errors, newline)
    except BaseException:
        stream.close()
        raise


if __name__ == "__main__":
    raise SystemExit("the gzip command-line interface is not supported")
