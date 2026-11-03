import errno
import os
import posix
import sys


def current():
    if sys.platform != "linux":
        return
    assert os.eventfd is posix.eventfd
    assert os.eventfd_read is posix.eventfd_read
    assert os.eventfd_write is posix.eventfd_write
    for extra in (0, os.EFD_CLOEXEC):
        fd = os.eventfd(initval=0, flags=os.EFD_NONBLOCK | extra)
        try:
            assert os.get_inheritable(fd) == (extra == 0)
            try:
                os.eventfd_read(fd)
            except BlockingIOError as err:
                assert err.errno == errno.EAGAIN
            else:
                raise AssertionError("empty nonblocking descriptor was readable")
            increment = 2**64 - 2
            assert os.eventfd_write(fd=fd, value=increment) is None
            assert os.eventfd_read(fd=fd) == increment
        finally:
            os.close(fd)
    fd = os.eventfd(2, os.EFD_SEMAPHORE | os.EFD_CLOEXEC)
    try:
        assert os.eventfd_read(fd) == os.eventfd_read(fd) == 1
    finally:
        os.close(fd)
