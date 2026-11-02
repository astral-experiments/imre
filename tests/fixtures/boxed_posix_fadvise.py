import errno
import os
import posix


def current():
    advise = os.posix_fadvise
    assert advise is posix.posix_fadvise
    path = "imre-posix-fadvise"
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
    try:
        os.write(fd, b"abc")
        for advice in (os.POSIX_FADV_NORMAL, os.POSIX_FADV_SEQUENTIAL, os.POSIX_FADV_RANDOM,
                       os.POSIX_FADV_WILLNEED, os.POSIX_FADV_DONTNEED, os.POSIX_FADV_NOREUSE):
            assert advise(fd, 1, 0, advice) is None
        assert os.lseek(fd, 0, os.SEEK_CUR) == 3
        os.lseek(fd, 0, os.SEEK_SET)
        assert os.read(fd, 4) == b"abc"
        try:
            advise(fd, 0, 1, -1)
        except OSError as exc:
            assert exc.errno == errno.EINVAL
        else:
            raise AssertionError("accepted invalid advice")
    finally:
        os.close(fd)
        os.unlink(path)

    try:
        advise(fd, 0, 0, posix.POSIX_FADV_NORMAL)
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("accepted a closed descriptor")
