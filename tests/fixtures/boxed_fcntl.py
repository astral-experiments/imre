import fcntl
import os
from fcntl import FD_CLOEXEC, F_GETPIPE_SZ, F_SETPIPE_SZ
from posix import O_PATH


class Descriptor:
    def __init__(self, fd):
        self.fd = fd

    def fileno(self):
        return self.fd


class Index:
    def __init__(self, n):
        self.n = n

    def __index__(self):
        return self.n


def current():
    query = fcntl.fcntl
    assert os.O_PATH == O_PATH
    fd = os.open("/dev/null", O_PATH)
    try:
        assert query(fd, fcntl.F_GETFD) & FD_CLOEXEC
        os.set_inheritable(fd, True)
        assert query(fd, fcntl.F_GETFD) & fcntl.FD_CLOEXEC == 0
    finally:
        os.close(fd)
    reader, writer = os.pipe()
    try:
        before = query(reader, F_GETPIPE_SZ)
        adjusted = query(Descriptor(writer), Index(F_SETPIPE_SZ), Index(2**80 + 1))
        assert 0 < adjusted <= before
        assert query(reader, F_GETPIPE_SZ) == query(writer, F_GETPIPE_SZ) == adjusted
        assert query(reader, F_SETPIPE_SZ, 1) == adjusted
        print("pipe capacity", before, adjusted)
        for arg in (b"", b"a\x00b", bytearray(b"raw"), memoryview(b"view"), "é"):
            result = query(reader, F_GETPIPE_SZ, arg)
            assert type(result) is bytes
            print(result)
        for get, set_ in ((fcntl.F_GETFD, fcntl.F_SETFD), (fcntl.F_GETFL, fcntl.F_SETFL)):
            flags = query(reader, get)
            assert query(reader, set_, flags) == 0
            assert query(reader, get) == flags
        for call in (
            lambda: query(),
            lambda: query(reader, F_GETPIPE_SZ, 0, 1),
            lambda: query(fd=reader, cmd=F_GETPIPE_SZ),
            lambda: query(-1, F_GETPIPE_SZ),
            lambda: query(Descriptor(None), F_GETPIPE_SZ),
            lambda: query(reader, 2**80),
            lambda: query(reader, F_GETPIPE_SZ, None),
            lambda: query(reader, F_GETPIPE_SZ, bytes(1025)),
        ):
            try:
                call()
            except (TypeError, ValueError, OverflowError) as exc:
                print(type(exc).__name__, str(exc))
            else:
                raise AssertionError("invalid argument accepted")
        fd = os.open("/dev/null", os.O_RDONLY)
        try:
            try:
                query(fd, F_GETPIPE_SZ)
            except OSError as exc:
                print("nonpipe", exc.errno)
            else:
                raise AssertionError("pipe capacity query on /dev/null succeeded")
        finally:
            os.close(fd)
    finally:
        os.close(reader)
        os.close(writer)
    try:
        query(reader, F_GETPIPE_SZ)
    except OSError as exc:
        print("closed", exc.errno)
    else:
        raise AssertionError("closed descriptor accepted")
