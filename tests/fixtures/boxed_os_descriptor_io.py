import errno
import posix
import sys

if sys.platform == "linux":
    tmpfile_flag = vars(posix)["O_TMPFILE"]

import os
from os import read, write
import tempfile


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


def expect_errno(operation, args, number):
    try:
        operation(*args)
    except OSError as error:
        assert error.errno == number
    else:
        raise AssertionError("descriptor operation unexpectedly succeeded")


def check_sendfile():
    from os import sendfile

    assert sendfile is os.sendfile and sendfile is posix.sendfile
    with tempfile.TemporaryFile() as source, tempfile.TemporaryFile() as destination:
        src = source.fileno()
        dst = destination.fileno()
        os.write(src, b"abcdef")
        os.lseek(src, 4, os.SEEK_SET)
        assert sendfile(out_fd=Index(dst), in_fd=Index(src), offset=Index(1), count=Index(2)) == 2
        assert os.lseek(src, 0, os.SEEK_CUR) == 4
        assert sendfile(dst, src, None, 0) == 0
        assert sendfile(dst, src, None, 10) == 2
        assert sendfile(dst, src, None, 1) == 0
        assert os.lseek(src, 0, os.SEEK_CUR) == 6
        os.lseek(dst, 0, os.SEEK_SET)
        assert os.read(dst, 10) == b"bcef"
        expect_errno(sendfile, (-1, src, None, 0), errno.EBADF)


def current():
    if sys.platform == "linux":
        check_tmpfile()
        check_sendfile()
    assert os.strerror is posix.strerror
    try:
        os.fstat(-1)
    except OSError as error:
        assert os.strerror(Index(error.errno)) == error.strerror
    else:
        raise AssertionError("fstat accepted a negative descriptor")
    assert os.read is posix.read and os.write is posix.write
    assert read is posix.read and write is posix.write
    if sys.platform == "linux" and sys.maxsize == 2**63 - 1:
        try:
            os.read(-1, sys.maxsize - 33)
        except MemoryError as error:
            assert error.args == ()
        else:
            raise AssertionError("oversized bytes allocation succeeded")
    with tempfile.TemporaryFile() as file:
        fd = file.fileno()
        duplicate = os.dup(fd)
        try:
            assert os.write(Index(fd), bytearray(b"\xff\x00b")) == 3
            assert os.lseek(duplicate, 0, os.SEEK_CUR) == 3
            os.lseek(duplicate, 0, os.SEEK_SET)
            assert os.read(Index(duplicate), Index(2)) == b"\xff\x00"
            assert os.read(fd, 0) == b""
            assert os.read(fd, 64) == b"b"
            assert os.read(duplicate, 1) == b""
            assert os.write(fd, b"") == 0
            expect_errno(os.read, (fd, -1), errno.EINVAL)
        finally:
            os.close(duplicate)
        expect_errno(os.read, (duplicate, 0), errno.EBADF)
        expect_errno(os.write, (duplicate, b""), errno.EBADF)
        assert os.lseek(fd, 0, os.SEEK_CUR) == 3

    reader, writer = os.pipe()
    try:
        try:
            os.set_blocking(reader, False)
            os.set_blocking(writer, False)
            try:
                os.read(reader, 1)
            except BlockingIOError:
                pass
            else:
                raise AssertionError("empty pipe was readable")

            # A write larger than the pipe must return its actual short count.
            chunk = b"\x01\xff" * 2048
            capacity = 0
            while True:
                try:
                    capacity += os.write(writer, chunk)
                except BlockingIOError:
                    break
            assert capacity >= len(chunk)
            payload = chunk * (capacity // len(chunk) + 2)
            assert os.read(reader, capacity + 1) == payload[:capacity]
            written = os.write(writer, payload)
            assert 0 < written < len(payload)
            assert os.read(reader, written + 1) == payload[:written]
        finally:
            os.close(writer)
        assert os.read(reader, 1) == b""
    finally:
        os.close(reader)
    print("borrowed descriptors, short transfers and EOF passed")


def check_tmpfile():
    assert os.O_TMPFILE is posix.O_TMPFILE is tmpfile_flag
    del posix.O_TMPFILE
    try:
        assert not hasattr(posix, "O_TMPFILE") and os.O_TMPFILE is tmpfile_flag
    finally:
        posix.O_TMPFILE = tmpfile_flag
    assert os.O_TMPFILE != os.O_DIRECTORY and os.O_TMPFILE & os.O_DIRECTORY
    with tempfile.TemporaryDirectory() as directory:
        try:
            tmp_fd = os.open(directory, os.O_TMPFILE | os.O_RDWR, 0o600)
        except OSError as error:
            # Linux can expose O_TMPFILE on a filesystem without anonymous file support.
            if error.errno not in (errno.EOPNOTSUPP, errno.EISDIR, errno.ENOENT):
                raise
        else:
            try:
                assert os.fstat(tmp_fd).st_nlink == 0 and not os.listdir(directory)
                assert os.write(tmp_fd, b"anonymous") == 9
                os.lseek(tmp_fd, 0, os.SEEK_SET)
                assert os.read(tmp_fd, 9) == b"anonymous"
            finally:
                os.close(tmp_fd)
