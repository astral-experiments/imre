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
READ_BUFFER_SIZE = 128 * 1024
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
            _imre_gzip.raw_rewind(self.decoder)
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
    def mtime(self):
        """Last modification time read from stream, or None."""
        if self.mode == WRITE:
            raise AttributeError("mtime is only available in read mode")
        if self._reader is None:
            return self._last_mtime
        return _imre_gzip.raw_mtime(self._reader.raw.decoder)

    @property
    def closed(self):
        return self._reader is None and self._writer is None

    def _check_open(self):
        if self.closed:
            raise ValueError("I/O operation on closed file")

    def _open_reader(self, operation="read"):
        self._check_open()
        if self._reader is None:
            raise OSError(errno.EBADF, f"{operation}() on write-only GzipFile object")
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

    def peek(self, n):
        return self._open_reader("peek").peek(n)

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
                self._last_mtime = _imre_gzip.raw_mtime(reader.raw.decoder)
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


# Adapted from CPython 3.14.5, Lib/gzip.py. See stdlib/docstrings/LICENSE.
_FHCRC, _FEXTRA, _FNAME, _FCOMMENT = 2, 4, 8, 16


def _read_exact(fp, n):
    '''Read exactly *n* bytes from `fp`

    This method is required because fp may be unbuffered,
    i.e. return short reads.
    '''
    data = fp.read(n)
    while len(data) < n:
        b = fp.read(n - len(data))
        if not b:
            raise EOFError("Compressed file ended before the "
                           "end-of-stream marker was reached")
        data += b
    return data


def _read_gzip_header(fp):
    '''Read a gzip header from `fp` and progress to the end of the header.

    Returns last mtime if header was present or None otherwise.
    '''
    magic = fp.read(2)
    if magic == b'':
        return None

    if magic != b'\037\213':
        raise BadGzipFile('Not a gzipped file (%r)' % magic)

    (method, flag, last_mtime) = struct.unpack("<BBIxx", _read_exact(fp, 8))
    if method != 8:
        raise BadGzipFile('Unknown compression method')

    if flag & _FEXTRA:
        # Read & discard the extra field, if present
        extra_len, = struct.unpack("<H", _read_exact(fp, 2))
        _read_exact(fp, extra_len)
    if flag & _FNAME:
        # Read and discard a null-terminated string containing the filename
        while True:
            s = fp.read(1)
            if not s or s==b'\000':
                break
    if flag & _FCOMMENT:
        # Read and discard a null-terminated string containing a comment
        while True:
            s = fp.read(1)
            if not s or s==b'\000':
                break
    if flag & _FHCRC:
        _read_exact(fp, 2)     # Read & discard the 16-bit header CRC
    return last_mtime


def decompress(data):
    """Decompress a gzip compressed string in one shot.
    Return the decompressed string.
    """
    decompressed_members = []
    while True:
        fp = io.BytesIO(data)
        if _read_gzip_header(fp) is None:
            return b"".join(decompressed_members)
        do = zlib.decompressobj(wbits=-zlib.MAX_WBITS)
        # Read all the data except the header
        decompressed = do.decompress(data[fp.tell():])
        if not do.eof or len(do.unused_data) < 8:
            raise EOFError("Compressed file ended before the end-of-stream "
                           "marker was reached")
        crc, length = struct.unpack("<II", do.unused_data[:8])
        if crc != zlib.crc32(decompressed):
            raise BadGzipFile("CRC check failed")
        if length != (len(decompressed) & 0xffffffff):
            raise BadGzipFile("Incorrect length of data produced")
        decompressed_members.append(decompressed)
        data = do.unused_data[8:].lstrip(b"\x00")


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


# Adapted from CPython 3.14.5, Lib/gzip.py. See stdlib/docstrings/LICENSE.
def main():
    import sys
    from argparse import ArgumentParser
    from contextlib import ExitStack

    parser = ArgumentParser(
        description="A simple command line interface for the gzip module: "
                    "act like gzip, but do not delete the input file.",
        color=True,
    )
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--fast", action="store_true", help="compress faster")
    group.add_argument("--best", action="store_true", help="compress better")
    group.add_argument("-d", "--decompress", action="store_true",
                       help="act like gunzip instead of gzip")
    parser.add_argument("args", nargs="*", default=["-"], metavar="file")
    args = parser.parse_args()

    compresslevel = 6
    if args.fast:
        compresslevel = 1
    elif args.best:
        compresslevel = 9

    for name in args.args:
        with ExitStack() as files:
            if args.decompress:
                if name == "-":
                    source = files.enter_context(
                        GzipFile(filename="", mode="rb", fileobj=sys.stdin.buffer)
                    )
                    target = sys.stdout.buffer
                else:
                    if not name.endswith(".gz"):
                        sys.exit(f"filename doesn't end in .gz: {name!r}")
                    source = files.enter_context(open(name, "rb"))
                    target = files.enter_context(io.open(name[:-3], "wb"))
            elif name == "-":
                source = sys.stdin.buffer
                target = files.enter_context(
                    GzipFile(filename="", mode="wb", fileobj=sys.stdout.buffer, compresslevel=compresslevel)
                )
            else:
                source = files.enter_context(io.open(name, "rb"))
                target = files.enter_context(open(name + ".gz", "wb"))

            while True:
                chunk = source.read(READ_BUFFER_SIZE)
                if not chunk:
                    break
                target.write(chunk)


if __name__ == "__main__":
    main()
