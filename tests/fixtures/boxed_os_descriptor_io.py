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


def check_fchdir():
    from os import fchdir

    assert fchdir is posix.fchdir and fchdir is os.fchdir
    previous = os.getcwd()
    with tempfile.TemporaryDirectory() as directory:
        parent = os.open(".", os.O_RDONLY)
        try:
            child = os.open(directory, os.O_RDONLY)
            try:
                assert fchdir(fd=child) is None
                assert os.path.samestat(os.stat("."), os.fstat(child))
                with tempfile.TemporaryFile() as file:
                    expect_errno(fchdir, (file,), errno.ENOTDIR)
                assert os.path.samestat(os.stat("."), os.fstat(child))
            finally:
                try:
                    fchdir(parent)
                finally:
                    os.close(child)
            expect_errno(fchdir, (child,), errno.EBADF)
        finally:
            os.close(parent)
    assert os.getcwd() == previous


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


def check_copy_file_range():
    from os import copy_file_range

    assert copy_file_range is os.copy_file_range and copy_file_range is posix.copy_file_range
    with tempfile.TemporaryFile() as source, tempfile.TemporaryFile() as destination:
        src = source.fileno()
        dst = destination.fileno()
        os.write(src, b"abcdef")
        os.write(dst, b"......")
        os.lseek(src, 4, os.SEEK_SET)
        os.lseek(dst, 0, os.SEEK_SET)
        assert copy_file_range(
            src=Index(src), dst=Index(dst), count=Index(2), offset_src=Index(1), offset_dst=Index(2)
        ) == 2
        assert os.lseek(src, 0, os.SEEK_CUR) == 4
        assert os.lseek(dst, 0, os.SEEK_CUR) == 0
        assert copy_file_range(src, dst, 0) == 0
        assert copy_file_range(src, dst, 10, None, None) == 2
        assert copy_file_range(src, dst, 1) == 0
        assert os.lseek(src, 0, os.SEEK_CUR) == 6
        assert os.lseek(dst, 0, os.SEEK_CUR) == 2
        os.lseek(dst, 0, os.SEEK_SET)
        assert os.read(dst, 10) == b"efbc.."
        expect_errno(copy_file_range, (-1, dst, 0), errno.EBADF)
        try:
            copy_file_range(src, dst, -1, object())
        except ValueError as error:
            assert str(error) == "negative value for 'count' not allowed"
        else:
            raise AssertionError("negative count was accepted")


def check_readv():
    from os import readv

    assert readv is os.readv is posix.readv
    with tempfile.TemporaryFile() as source:
        fd = source.fileno()
        os.write(fd, b"abcdef")
        os.lseek(fd, 1, os.SEEK_SET)
        first, last = bytearray(b".."), bytearray(b"....")
        assert readv(fd, [first, last]) == 5
        assert (first, last) == (b"bc", b"def.")
        assert os.lseek(fd, 0, os.SEEK_CUR) == 6
        assert readv(fd, [last]) == 0
        assert readv(fd, []) == 0
        os.lseek(fd, 0, os.SEEK_SET)
        storage = bytearray(b"......")
        view = memoryview(storage)
        assert readv(fd, (view[:3], view[1:4])) == 6
        assert storage == b"adef.."
        os.lseek(fd, 0, os.SEEK_SET)
        try:
            readv(fd, [first, b"readonly"])
        except BufferError:
            pass
        else:
            raise AssertionError("readv accepted a read-only buffer")
        assert os.lseek(fd, 0, os.SEEK_CUR) == 0
        first.append(0)
    expect_errno(readv, (fd, [first]), errno.EBADF)


def check_preadv():
    from os import preadv

    assert preadv is os.preadv is posix.preadv
    assert os.RWF_HIPRI == posix.RWF_HIPRI
    assert os.RWF_NOWAIT == posix.RWF_NOWAIT
    with tempfile.TemporaryFile() as source:
        fd = source.fileno()
        os.write(fd, b"abcdef")
        os.lseek(fd, 1, os.SEEK_SET)
        head, tail = bytearray(b".."), bytearray(b"...")
        assert preadv(fd, [head, tail], 2) == 4
        assert (head, tail) == (b"cd", b"ef.")
        assert preadv(fd, [head], 6, 0) == 0
        expect_errno(preadv, (fd, [head], -2), errno.EINVAL)
        assert os.lseek(fd, 0, os.SEEK_CUR) == 1
        try:
            preadv(fd, [head], 0, 1 << 30)
        except NotImplementedError:
            pass  # libc supplies only the flags-free preadv.
        except OSError as error:
            assert error.errno in (errno.EINVAL, errno.EOPNOTSUPP)
        else:
            raise AssertionError("unsupported preadv flag succeeded")
        assert os.lseek(fd, 0, os.SEEK_CUR) == 1
    expect_errno(preadv, (fd, [head], 0), errno.EBADF)


def check_writev():
    from os import writev

    assert writev is os.writev is posix.writev
    data = bytearray(b"abc")
    with tempfile.TemporaryFile() as target:
        fd = target.fileno()
        parts = (b"<", data, memoryview(data)[1:], memoryview(b"|>"))
        assert writev(fd, parts) == 8
        assert os.lseek(fd, 0, os.SEEK_CUR) == 8
        assert os.pread(fd, 8, 0) == b"<abcbc|>"
        assert writev(fd, []) == 0
        probe = bytearray(b"unchanged")
        try:
            writev(fd, [probe, object()])
        except TypeError:
            pass
        else:
            raise AssertionError("writev accepted a non-buffer")
        probe.append(0)
        assert os.lseek(fd, 0, os.SEEK_CUR) == 8
    expect_errno(writev, (fd, [b"x"]), errno.EBADF)

    reader, writer = os.pipe()
    os.close(reader)
    try:
        expect_errno(writev, (writer, [b"x", b"y"]), errno.EPIPE)
    finally:
        os.close(writer)


def check_pwritev():
    from os import pwritev

    assert pwritev is os.pwritev is posix.pwritev
    with tempfile.TemporaryFile() as target:
        fd = target.fileno()
        os.write(fd, b"abcdef")
        os.lseek(fd, 1, os.SEEK_SET)
        assert pwritev(fd, [b"X", bytearray(b"Y"), memoryview(b"Z")], 2) == 3
        assert os.pread(fd, 6, 0) == b"abXYZf"
        expect_errno(pwritev, (fd, [b"!"], -2), errno.EINVAL)
        assert os.lseek(fd, 0, os.SEEK_CUR) == 1
        if hasattr(os, "RWF_APPEND"):
            try:
                n = pwritev(fd, [b"!", b"?"], 0, os.RWF_APPEND)
            except NotImplementedError:
                pass  # The linked libc implements only pwritev.
            except OSError as error:
                assert error.errno in (errno.EINVAL, errno.EOPNOTSUPP)
            else:
                assert n == 2 and os.pread(fd, 8, 0) == b"abXYZf!?"
                assert os.lseek(fd, 0, os.SEEK_CUR) == 1
                assert pwritev(fd, [b"+"], -1, os.RWF_APPEND) == 1
                assert os.lseek(fd, 0, os.SEEK_CUR) == 9
                assert os.pread(fd, 9, 0) == b"abXYZf!?+"


def check_splice():
    from os import splice

    assert splice is os.splice is posix.splice
    for name in ("SPLICE_F_MOVE", "SPLICE_F_MORE", "SPLICE_F_NONBLOCK"):
        assert vars(os)[name] == vars(posix)[name]
    with tempfile.TemporaryFile() as source, tempfile.TemporaryFile() as destination:
        src, dst = source.fileno(), destination.fileno()
        os.write(src, b"abcdef")
        reader, writer = os.pipe()
        try:
            flags = os.SPLICE_F_MOVE | os.SPLICE_F_MORE
            assert splice(src=Index(src), dst=Index(writer), count=Index(4),
                          flags=Index(flags), offset_src=Index(1)) == 4
            assert os.lseek(src, 0, os.SEEK_CUR) == 6
            assert splice(reader, dst, 2, offset_dst=3) == 2
            assert os.read(reader, 2) == b"de"
            assert os.lseek(dst, 0, os.SEEK_CUR) == 0
            assert os.read(dst, 5) == b"\x00\x00\x00bc"
            expect_errno(splice, (reader, dst, 1, None, None, os.SPLICE_F_NONBLOCK), errno.EAGAIN)
            expect_errno(splice, (reader, dst, 1, 0), errno.ESPIPE)
            assert splice(-1, -1, 0) == 0
        finally:
            os.close(reader)
            os.close(writer)


def current():
    check_fchdir()
    if sys.platform == "linux":
        check_tmpfile()
        check_copy_file_range()
        check_sendfile()
        check_splice()
        check_readv()
        check_preadv()
        check_writev()
        check_pwritev()
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
