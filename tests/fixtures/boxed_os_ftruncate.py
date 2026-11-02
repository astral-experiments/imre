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


exercise()
