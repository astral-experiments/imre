import errno
import os
import posix
import tempfile
from os import pread


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


def check_errno(fd, length, offset, expected):
    try:
        pread(fd, length, offset)
    except OSError as error:
        assert error.errno == expected
    else:
        raise AssertionError("pread did not raise OSError")


def exercise():
    assert pread is os.pread is posix.pread
    with tempfile.TemporaryFile() as stream:
        fd = stream.fileno()
        os.write(fd, b"abc\x00\xff")
        os.lseek(fd, 1, os.SEEK_SET)
        dup = os.dup(fd)
        try:
            data = os.pread(Index(dup), Index(8), Index(3))
            assert type(data) is bytes and data == b"\x00\xff"
            assert posix.pread(fd, 2, 8) == b""
            assert pread(fd, 0, 0) == b""
            check_errno(fd, -1, 0, errno.EINVAL)
            check_errno(fd, 1, -1, errno.EINVAL)
            assert os.read(dup, 2) == b"bc"
            assert os.lseek(fd, 0, os.SEEK_CUR) == 3
        finally:
            os.close(dup)
        check_errno(dup, 0, 0, errno.EBADF)

    reader, writer = os.pipe()
    try:
        check_errno(reader, 1, 0, errno.ESPIPE)
    finally:
        os.close(reader)
        os.close(writer)


exercise()
