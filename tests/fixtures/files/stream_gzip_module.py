import errno
import importlib
import os


class ReadablePath:
    def __fspath__(self):
        return "rows.gz"

    def read(self, size=-1):
        raise AssertionError("gzip.open treated a path as a file object")


def assert_fd_closed(fd):
    try:
        os.fstat(fd)
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("gzip.open left its file descriptor open")


gzip = importlib.import_module("gzip")
assert importlib.import_module("gzip") is gzip
assert issubclass(gzip.BadGzipFile, OSError)
assert gzip.BadGzipFile("bad member").args == ("bad member",)
with gzip.open("invalid-text.gz", "rb") as single:
    assert single.read() == b"bad\xff"

with open("rows.txt", "rb") as plain:
    expected = plain.read()
with gzip.open(ReadablePath(), "rt", encoding="utf-8") as text:
    assert text.read() == expected.decode("utf-8").replace("\r\n", "\n").replace("\r", "\n")
with gzip.open("rows.gz", "rb") as members:
    fd = members.fileno()
    assert members.read(7) + members.read() == expected
    members.flush()
assert members.closed
members.close()
assert_fd_closed(fd)
try:
    members.readline(0)
except ValueError:
    pass
else:
    raise AssertionError("readline(0) accepted a closed reader")

members.__init__("invalid-text.gz", "rb")
with members:
    fd = members.fileno()
    assert members.read() == b"bad\xff"
assert_fd_closed(fd)

try:
    with gzip.open("crc.gz", "rb") as corrupt:
        corrupt.read()
except OSError as exc:
    assert type(exc) is gzip.BadGzipFile
else:
    raise AssertionError("bad gzip checksum accepted")
assert corrupt.closed

try:
    with gzip.open("short.gz", "rb") as truncated:
        truncated.read()
except EOFError:
    pass
else:
    raise AssertionError("truncated gzip trailer accepted")
assert truncated.closed
