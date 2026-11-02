import errno
import os
import posix
import sys
import tempfile


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


def exercise():
    events = []

    def audit(name, args):
        if name == "os.truncate":
            events.append(args)
            if args[1] == 7:
                raise RuntimeError("truncate refused")

    sys.addaudithook(audit)
    assert os.ftruncate is posix.ftruncate
    assert vars(os)["truncate"] is os.truncate is posix.truncate
    assert os.truncate in os.supports_fd
    with tempfile.TemporaryFile() as stream:
        fd = stream.fileno()
        os.write(fd, b"abcdef")
        assert os.ftruncate(Index(fd), Index(3)) is None
        assert os.lseek(fd, 0, os.SEEK_CUR) == 6
        os.ftruncate(fd, 5)
        try:
            os.ftruncate(fd, 7)
        except RuntimeError as error:
            assert str(error) == "truncate refused"
        else:
            raise AssertionError("audit hook did not prevent truncation")
        assert events == [(fd, 3), (fd, 5), (fd, 7)]
        os.lseek(fd, 0, os.SEEK_SET)
        assert os.read(fd, 10) == b"abc\0\0"
        try:
            os.ftruncate(fd, -1)
        except OSError as error:
            assert error.errno == errno.EINVAL
        else:
            raise AssertionError("accepted negative length")
    try:
        os.ftruncate(fd, 0)
    except OSError as error:
        assert error.errno == errno.EBADF
    else:
        raise AssertionError("accepted closed descriptor")

    check_path(events)
    if sys.platform == "linux":
        sentinel = Index(-1)
        for length, code in ((-1, errno.EINVAL), (0, errno.EFAULT)):
            try:
                os.truncate(sentinel, length)
            except OSError as error:
                assert error.errno == code and error.filename is sentinel
            else:
                raise AssertionError("truncate accepted fd -1")
            assert events[-1] == (sentinel, length)


def check_path(events):
    name = b"imre-boxed-truncate-\xff"

    class Path:
        def __fspath__(self):
            return name

    fd = os.open(name, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
    try:
        os.write(fd, b"abcde")
        assert os.truncate(length=Index(2), path=Path()) is None
        assert events[-1][0] is name and events[-1][1] == 2
        assert posix.truncate(Index(fd), 4) is None
        assert events[-1] == (fd, 4)
        try:
            os.truncate(name, 7)
        except RuntimeError as error:
            assert str(error) == "truncate refused"
        else:
            raise AssertionError("audit hook did not prevent pathname truncation")
        assert os.lseek(fd, 0, os.SEEK_CUR) == 5
        os.lseek(fd, 0, os.SEEK_SET)
        assert os.read(fd, 6) == b"ab\0\0"
    finally:
        os.close(fd)
        os.unlink(name)
    try:
        os.truncate(Path(), 0)
    except FileNotFoundError as error:
        assert error.filename is name
    else:
        raise AssertionError("truncated a missing file")


exercise()
