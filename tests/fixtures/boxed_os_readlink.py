import os
import posix
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
