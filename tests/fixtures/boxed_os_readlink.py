import gc
import os
import posix
import weakref
from os import readlink
from posix import readlink as posix_readlink


class Path:
    def __init__(self, value):
        self.value = value

    def __fspath__(self):
        return self.value


def current():
    assert readlink is os.readlink is posix_readlink is posix.readlink
    assert readlink in os.supports_dir_fd and readlink not in os.supports_fd
    assert readlink("readlink-source") == os.fsdecode(b"unresolved-\xff")
    fd = os.open(".", os.O_RDONLY)
    try:
        assert readlink(path=Path(b"readlink-source"), dir_fd=fd) == b"unresolved-\xff"
    finally:
        os.close(fd)
    missing = b"missing-\xfe"
    try:
        readlink(Path(missing))
    except FileNotFoundError as error:
        assert error.filename is missing and error.filename2 is None
    else:
        raise AssertionError("readlink accepted missing path")

    class Spelling(str):
        pass

    spellings = [Spelling("missing-before-collect")]
    ref = weakref.ref(spellings[0])

    class PoppedPath:
        __fspath__ = staticmethod(spellings.pop)

    class ClosedFd:
        def __index__(self):
            gc.collect()
            return -2

    try:
        readlink(PoppedPath(), dir_fd=ClosedFd())
    except OSError as error:
        assert type(error.filename) is Spelling and error.filename is ref()
    else:
        raise AssertionError("readlink accepted closed directory descriptor")
