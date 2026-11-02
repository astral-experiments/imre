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
    assert os.lockf is posix.lockf
    assert (os.F_LOCK, os.F_TLOCK, os.F_ULOCK, os.F_TEST) == (
        posix.F_LOCK, posix.F_TLOCK, posix.F_ULOCK, posix.F_TEST
    )
    events = []

    def audit(name, args):
        if name == "os.lockf":
            events.append(args)
            if args[2] == 99:
                raise RuntimeError("lock denied")

    sys.addaudithook(audit)
    with tempfile.TemporaryFile() as file:
        fd = file.fileno()
        os.write(fd, b"abcd")
        assert posix.lockf(Index(fd), Index(os.F_LOCK), Index(-2)) is None
        assert events == [(fd, os.F_LOCK, -2)]
        assert all(type(arg) is int for arg in events[0])
        assert os.lockf(fd, os.F_ULOCK, -2) is None
        assert os.lockf(fd, os.F_TLOCK, 0) is None
        assert os.lockf(fd, os.F_TEST, 0) is None
        assert os.lockf(fd, os.F_ULOCK, 0) is None
        assert os.lseek(fd, 0, os.SEEK_CUR) == 4
        assert os.pread(fd, 4, 0) == b"abcd"
        try:
            os.lockf(fd, -1, 2)
        except OSError as exc:
            assert exc.errno == errno.EINVAL
        else:
            raise AssertionError("accepted invalid lock command")
    try:
        os.lockf(fd, os.F_TEST, 1)
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("queried a closed descriptor")
    try:
        os.lockf(fd, os.F_TEST, 99)
    except RuntimeError as exc:
        assert str(exc) == "lock denied"
    else:
        raise AssertionError("audit hook did not veto the call")


if sys.platform == "linux":
    exercise()
