import errno
import os
import posix
import stat
import sys
import tempfile


class Path:
    def __fspath__(self):
        return b"pipe"


def exercise():
    assert os.mknod is posix.mknod
    directory = tempfile.mkdtemp()
    fd = os.open(directory, os.O_RDONLY)
    try:
        assert os.mknod(path=directory + "/regular") is None
        assert stat.S_ISREG(os.stat("regular", dir_fd=fd).st_mode)
        assert posix.mknod(Path(), stat.S_IFIFO | 0o600, device=0, dir_fd=fd) is None
        assert stat.S_ISFIFO(os.stat(b"pipe", dir_fd=fd).st_mode)
        try:
            posix.mknod(b"pipe", dir_fd=fd)
        except FileExistsError as exc:
            assert exc.errno == errno.EEXIST and exc.filename is None
        else:
            raise AssertionError("recreated the FIFO")
    finally:
        for name in ("regular", "pipe"):
            try:
                os.unlink(name, dir_fd=fd)
            except FileNotFoundError:
                pass
        os.close(fd)
        os.rmdir(directory)
    try:
        os.mknod("missing", dir_fd=fd)
    except OSError as exc:
        assert exc.errno == errno.EBADF and exc.filename is None
    else:
        raise AssertionError("accepted a closed directory descriptor")


if sys.platform == "linux":
    exercise()
