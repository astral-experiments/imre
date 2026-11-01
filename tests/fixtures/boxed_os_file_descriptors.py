import os
import posix
import sys


def current():
    fd = os.open(path=__file__, flags=os.O_RDONLY)
    try:
        assert not os.get_inheritable(fd) and not os.isatty(fd)
        duplicate = os.dup(fd)
        try:
            assert not os.get_inheritable(duplicate)
            assert os.lseek(duplicate, 5, os.SEEK_SET) == 5
            assert os.lseek(fd, 0, os.SEEK_CUR) == 5
            assert posix.dup2(fd, duplicate) == duplicate
            assert os.get_inheritable(duplicate)
            assert os.dup2(fd=fd, fd2=duplicate, inheritable=False) == duplicate
            assert not os.get_inheritable(duplicate)
        finally:
            os.close(duplicate)
    finally:
        os.close(fd)

    class Path:
        def __fspath__(self):
            return missing

    raw = b"imre-missing-file/\xff"
    assert sys.getfilesystemencoding() is sys.getfilesystemencoding()
    errors = sys.getfilesystemencodeerrors()
    assert errors is sys.getfilesystemencodeerrors() and errors == "surrogateescape"
    assert raw.decode(sys.getfilesystemencoding(), errors) == os.fsdecode(raw)
    for missing in (raw, os.fsdecode(raw)):
        assert os.fspath(Path()) is missing
        assert os.fsencode(Path()) == raw
        try:
            os.open(Path(), os.O_RDONLY, dir_fd=None)
        except FileNotFoundError as error:
            assert error.filename is missing
            print(error.errno, type(error.filename).__name__)
        else:
            raise AssertionError("opened a missing file")
