import errno
import os
import posix


def current():
    allocate = os.posix_fallocate
    assert allocate is posix.posix_fallocate
    path = "imre-posix-fallocate"
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        os.write(fd, b"abc")
        assert allocate(fd, 2, 14) is None
        assert allocate(fd, 0, 2) is None
        assert os.fstat(fd).st_size == 16
        assert os.lseek(fd, 0, os.SEEK_CUR) == 3
        os.lseek(fd, 0, os.SEEK_SET)
        assert os.read(fd, 5) == b"abc\0\0"
        try:
            allocate(fd, 0, 0)
        except OSError as exc:
            assert exc.errno == errno.EINVAL
        else:
            raise AssertionError("allocated an empty range")
    finally:
        os.close(fd)
        os.unlink(path)

    try:
        allocate(fd, 0, 1)
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("allocated on a closed descriptor")
