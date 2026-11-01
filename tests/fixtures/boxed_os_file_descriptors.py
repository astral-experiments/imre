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

    from posix import F_OK, R_OK, W_OK, X_OK

    assert (F_OK, R_OK, W_OK, X_OK) == (0, 4, 2, 1)
    assert (os.F_OK, os.R_OK, os.W_OK, os.X_OK) == (F_OK, R_OK, W_OK, X_OK)

    def which(cmd, mode=os.F_OK | os.X_OK, path=None):
        return mode

    assert which("python") == X_OK

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

    directory = os.open(".", os.O_RDONLY)
    path = "imre-boxed-unlink"
    try:
        for remove, data in ((os.remove, b"hello"), (posix.unlink, b"Hello World!")):
            with open(path, "xb") as file:
                assert file.write(data) == len(data)
            assert remove(path=path, dir_fd=directory) is None
            try:
                remove(os.fsencode(path), dir_fd=directory)
            except FileNotFoundError as error:
                assert error.errno == 2 and error.filename == os.fsencode(path)
            else:
                raise AssertionError("file still exists after unlink")
    finally:
        os.close(directory)

    assert os.rmdir is posix.rmdir
    try:
        os.rmdir(__file__)
    except NotADirectoryError as error:
        assert error.filename == __file__
    else:
        raise AssertionError("rmdir removed a regular file")
