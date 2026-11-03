import errno
import os
import posix
import select
import sys
import time


def current():
    if sys.platform != "linux":
        return
    assert os.timerfd_create is posix.timerfd_create
    assert os.timerfd_settime is posix.timerfd_settime
    assert os.timerfd_settime_ns is posix.timerfd_settime_ns
    assert os.timerfd_gettime is posix.timerfd_gettime
    assert os.timerfd_gettime_ns is posix.timerfd_gettime_ns

    class IndexedClock:
        def __index__(self):
            return time.CLOCK_REALTIME

    fd = os.timerfd_create(IndexedClock(), flags=os.TFD_NONBLOCK | os.TFD_CLOEXEC)
    try:
        assert not os.get_inheritable(fd)
        assert os.timerfd_gettime(fd) == (0.0, 0.0)
        try:
            os.read(fd, 8)
        except BlockingIOError as exc:
            assert exc.errno == errno.EAGAIN
        else:
            raise AssertionError("a disarmed timer was readable")

        class FileDescriptor:
            def fileno(self):
                return fd

        handle = FileDescriptor()
        now = time.clock_gettime_ns(IndexedClock())
        assert isinstance(time.clock_gettime(IndexedClock()), float)
        assert isinstance(time.perf_counter_ns(), int)
        assert os.timerfd_settime_ns(
            handle, flags=os.TFD_TIMER_ABSTIME, initial=now
        ) == (0, 0)
        assert select.select([fd], [], [], 5)[0] == [fd]
        assert int.from_bytes(os.read(fd, 8), sys.byteorder) == 1
        assert os.timerfd_gettime_ns(handle) == (0, 0)
        assert os.timerfd_settime(handle) == (0.0, 0.0)
    finally:
        os.close(fd)

    try:
        os.timerfd_gettime(fd)
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("queried a closed timer descriptor")
