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


class _GzipRawReader(io.RawIOBase):
    def __init__(self, fileobj):
        self.fileobj = fileobj
        self.decoder = _imre_gzip.decoder()
        self._size = None

    def readable(self):
        return True

    def _read(self, size):
        data = _imre_gzip.raw_read(self.decoder, self.fileobj, size)
        if size and not data:
            self._size = self.tell()
        return data

    def readinto(self, buf):
        data = self._read(len(buf))
        buf[:len(data)] = data
        return len(data)

    def tell(self):
        return _imre_gzip.raw_tell(self.decoder)

    def seekable(self):
        # Forward seeks only need read(); a rewind uses the source's seek().
        return True

    def seek(self, offset, whence=io.SEEK_SET):
        if whence == io.SEEK_CUR:
            offset += self.tell()
        elif whence == io.SEEK_END:
            if self._size is None:
                while self._read(io.DEFAULT_BUFFER_SIZE):
                    pass
            offset += self._size
        elif whence != io.SEEK_SET:
            raise ValueError(f"Invalid value for whence: {whence}")

        position = self.tell()
        if offset < position:
            self.fileobj.seek(0)
            self.decoder = _imre_gzip.decoder()
            position = 0
        while position < offset:
            data = self._read(min(io.DEFAULT_BUFFER_SIZE, offset - position))
            if not data:
                break
            position += len(data)
        return position


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
            if mode.startswith("r"):
                mode = "rb"
            elif mode.startswith(("w", "a", "x")):
                import warnings
                warnings.warn(
                    "GzipFile was opened for writing, but this will change in future Python releases.  "
                    "Specify the mode argument for opening it for writing.",
                    FutureWarning, 2,
                )
        writing = mode in ("w", "wb", "a", "ab", "x", "xb")
        if not writing and mode not in (None, "r", "rb"):
            raise ValueError(f"Invalid mode: {mode!r}")
        if filename is None and fileobj is not None:
            name = getattr(fileobj, "name", "")
            if not isinstance(name, (str, bytes)):
                name = ""
        else:
            name = os.fspath(filename)
        if writing:
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
        owned = fileobj is None
        if owned:
            fileobj = io.open(name, "rb")
        try:
            reader = io.BufferedReader(_GzipRawReader(fileobj))
            if not self.closed:
                self.close()
        except BaseException:
            if owned:
                fileobj.close()
            raise
        self.mode = READ
        self.name = name
        self.fileobj = fileobj
        self.myfileobj = fileobj if owned else None
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
        return self._open_reader().read(-1 if size is None else size)

    def read1(self, size=-1):
        reader = self._open_reader()
        return reader.read1(io.DEFAULT_BUFFER_SIZE if size < 0 else size)

    def readline(self, size=-1):
        return self._open_reader().readline(size)

    def rewind(self):
        if self.mode != READ:
            raise OSError("Can't rewind in write mode")
        self._open_reader().seek(0)

    def seekable(self):
        return True

    def tell(self):
        self._check_open()
        if self.mode == READ:
            return self._reader.tell()
        self._writer.flush()
        return self._writer.raw.size

    def seek(self, offset, whence=io.SEEK_SET):
        if self.mode != WRITE:
            return self._open_reader().seek(offset, whence)
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
        return self.fileobj.fileno()

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
        reader = self._reader
        writer = self._writer
        if reader is None and writer is None:
            return
        try:
            if reader is not None:
                self._reader = None
                reader.close()
            else:
                self._writer = None
                raw = writer.raw
                writer.close()
                raw.flush_compressor(_Z_FINISH)
                self.fileobj.write(struct.pack("<II", raw.crc, raw.size & 0xffffffff))
        finally:
            self.fileobj = None
            if self.myfileobj is not None:
                owned = self.myfileobj
                self.myfileobj = None
                owned.close()


def compress(data, compresslevel=9, *, mtime=0):
    """Compress data into a single gzip member, with a reproducible timestamp by default."""
    member = zlib.compress(data, level=compresslevel, wbits=31)
    if mtime is None:
        mtime = time.time()
    # Keep zlib's compression flags and use gzip's portable OS byte.
    header = struct.pack("<4sLBB", member, int(mtime), member[8], 255)
    return header + member[10:]


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
