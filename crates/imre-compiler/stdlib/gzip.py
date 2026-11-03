"""Read and write gzip-compressed files."""

import errno
import io
import os
import struct
import time
import zlib
import _imre_gzip
from _imre_gzip import BadGzipFile


READ = "rb"
WRITE = "wb"
_Z_SYNC_FLUSH = 2
_Z_FINISH = 4
_WRITE_BUFFER_SIZE = 4 * io.DEFAULT_BUFFER_SIZE


def _write_header(fileobj, name, level, mtime):
    name = os.path.basename(name)
    if isinstance(name, str):
        try:
            name = name.encode("latin-1")
        except UnicodeEncodeError:
            name = b""
    if name.endswith(b".gz"):
        name = name[:-3]
    timestamp = int(time.time() if mtime is None else mtime)
    xfl = 2 if level == 9 else 4 if level == 1 else 0
    fileobj.write(struct.pack("<2sBBIBB", b"\x1f\x8b", 8, 8 if name else 0, timestamp, xfl, 255))
    if name:
        fileobj.write(name + b"\0")


class _WriteBuffer(io.RawIOBase):
    def __init__(self, fileobj, level):
        self.fileobj = fileobj
        self.encoder = _imre_gzip.writer(level)
        self.crc = 0
        self.size = 0

    def writable(self):
        return True

    def write(self, data):
        length = memoryview(data).nbytes
        if length:
            self.fileobj.write(_imre_gzip.compress(self.encoder, data))
            self.crc = zlib.crc32(data, self.crc)
            self.size += length
        return length

    def flush_compressor(self, mode):
        self.fileobj.write(_imre_gzip.flush_writer(self.encoder, mode))


class GzipFile(io.BufferedIOBase):
    """A binary file object for gzip-compressed data."""

    mode = READ
    fileobj = None
    _reader = None
    _writer = None
    myfileobj = None

    def __init__(self, filename=None, mode=None, compresslevel=9, fileobj=None, mtime=None):
        if mode is None and fileobj is not None:
            mode = getattr(fileobj, "mode", "rb")
            if mode.startswith(("w", "a", "x")):
                import warnings
                warnings.warn(
                    "GzipFile was opened for writing, but this will change in future Python releases.  "
                    "Specify the mode argument for opening it for writing.",
                    FutureWarning, 2,
                )
        if mode in ("w", "wb", "a", "ab", "x", "xb"):
            if filename is None and fileobj is not None:
                name = getattr(fileobj, "name", "")
                if not isinstance(name, (str, bytes)):
                    name = ""
            else:
                name = os.fspath(filename)
            owned = fileobj is None
            if owned:
                fileobj = io.open(name, mode[0] + "b")
            try:
                raw = _WriteBuffer(fileobj, compresslevel)
                buffer = io.BufferedWriter(raw, _WRITE_BUFFER_SIZE)
                if not self.closed:
                    self.close()
                _write_header(fileobj, name, compresslevel, mtime)
            except BaseException:
                if owned:
                    fileobj.close()
                raise
            self.mode = WRITE
            self.name = name
            self.fileobj = fileobj
            self.myfileobj = fileobj if owned else None
            self._writer = buffer
            return
        if mode not in (None, "r", "rb"):
            raise ValueError(f"Invalid mode: {mode!r}")
        if fileobj is not None:
            raise io.UnsupportedOperation("gzip reading from a file object is not supported")
        path = os.fspath(filename)
        reader = _imre_gzip.open(path)
        if not self.closed:
            self.close()
        self.mode = READ
        self.name = path
        self._reader = reader

    @property
    def closed(self):
        return self._reader is None and self._writer is None

    def _check_open(self):
        if self.closed:
            raise ValueError("I/O operation on closed file")

    def _open_reader(self):
        self._check_open()
        if self._reader is None:
            raise OSError(errno.EBADF, "read() on write-only GzipFile object")
        return self._reader

    def readable(self):
        return self.mode == READ

    def writable(self):
        return self.mode == WRITE

    def write(self, data):
        self._check_open()
        if self._writer is None:
            raise OSError(errno.EBADF, "write() on read-only GzipFile object")
        return self._writer.write(data)

    def read(self, size=-1):
        return _imre_gzip.read(self._open_reader(), -1 if size is None else size)

    def read1(self, size=-1):
        reader = self._open_reader()
        return _imre_gzip.read1(reader, io.DEFAULT_BUFFER_SIZE if size < 0 else size)

    def readline(self, size=-1):
        return _imre_gzip.readline(self._open_reader(), size)

    def seekable(self):
        if self.mode == WRITE:
            return True
        return super().seekable()

    def tell(self):
        self._check_open()
        if self.mode == READ:
            return _imre_gzip.tell(self._reader)
        self._writer.flush()
        return self._writer.raw.size

    def seek(self, offset, whence=io.SEEK_SET):
        if self.mode != WRITE:
            return super().seek(offset, whence)
        position = self.tell()
        if whence == io.SEEK_CUR:
            offset += position
        elif whence != io.SEEK_SET:
            raise ValueError("Seek from end not supported")
        if offset < position:
            raise OSError("Negative seek in write mode")
        count = offset - position
        if count >= _WRITE_BUFFER_SIZE:
            block = bytes(_WRITE_BUFFER_SIZE)
            for _ in range(count // _WRITE_BUFFER_SIZE):
                self.write(block)
        self.write(bytes(count % _WRITE_BUFFER_SIZE))
        # Buffered padding does not advance the compressor until it is drained.
        return self._writer.raw.size

    def fileno(self):
        if self.mode == WRITE:
            return self.fileobj.fileno()
        return _imre_gzip.fileno(self._open_reader())

    def flush(self, zlib_mode=_Z_SYNC_FLUSH):
        self._check_open()
        if self._writer is not None:
            self._writer.flush()
            self._writer.raw.flush_compressor(zlib_mode)
            self.fileobj.flush()

    def __enter__(self):
        self._check_open()
        return self

    def close(self):
        writer = self._writer
        if writer is not None:
            self._writer = None
            raw = writer.raw
            try:
                writer.close()
                raw.flush_compressor(_Z_FINISH)
                self.fileobj.write(struct.pack("<II", raw.crc, raw.size & 0xffffffff))
            finally:
                self.fileobj = None
                if self.myfileobj is not None:
                    owned = self.myfileobj
                    self.myfileobj = None
                    owned.close()
            return
        reader = self._reader
        if reader is not None:
            self._reader = None
            _imre_gzip.close(reader)


def open(filename, mode="rb", compresslevel=9, encoding=None, errors=None, newline=None):
    """Open a gzip-compressed path or file object in binary or text mode."""
    if "t" in mode:
        if "b" in mode:
            raise ValueError(f"Invalid mode: {mode!r}")
    elif encoding is not None or errors is not None or newline is not None:
        raise ValueError("encoding, errors and newline are only supported in text mode")
    if isinstance(filename, (str, bytes, os.PathLike)):
        stream = GzipFile(filename, mode.replace("t", ""), compresslevel)
    elif hasattr(filename, "read") or hasattr(filename, "write"):
        stream = GzipFile(mode=mode.replace("t", ""), compresslevel=compresslevel, fileobj=filename)
    else:
        raise TypeError("filename must be a str or bytes object, or a file")
    if "t" not in mode:
        return stream
    try:
        return io.TextIOWrapper(stream, encoding, errors, newline)
    except BaseException:
        stream.close()
        raise


if __name__ == "__main__":
    raise SystemExit("the gzip command-line interface is not supported")
