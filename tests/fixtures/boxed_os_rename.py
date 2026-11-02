import errno
import os
import posix
import sys


def current():
    assert os.rename is posix.rename and os.replace is posix.replace
    assert os.rename in os.supports_dir_fd and os.replace not in os.supports_dir_fd
    directory = "rename-fixture"
    old = b"old"
    new = b"new"
    os.mkdir(directory)
    fd = os.open(directory, os.O_RDONLY)
    events = []

    def audit(event, args):
        if event == "os.rename":
            events.append(args)

    sys.addaudithook(audit)
    try:
        os.symlink("missing", old, dir_fd=fd)
        inode = os.lstat(old, dir_fd=fd).st_ino
        os.close(os.open(new, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600, dir_fd=fd))
        assert os.replace(old, new, src_dir_fd=fd, dst_dir_fd=fd) is None
        assert os.readlink(new, dir_fd=fd) == b"missing"
        assert os.lstat(new, dir_fd=fd).st_ino == inode
        assert events[-1] == (old, new, fd, fd)
        try:
            os.rename(old, new, src_dir_fd=fd, dst_dir_fd=fd)
        except FileNotFoundError as error:
            assert error.errno == errno.ENOENT
            assert error.filename is old and error.filename2 is new
        else:
            raise AssertionError("renamed a missing source")
    finally:
        for name in (old, new):
            try:
                os.unlink(name, dir_fd=fd)
            except FileNotFoundError:
                pass
        os.close(fd)
        os.rmdir(directory)
