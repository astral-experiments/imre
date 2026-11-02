import os
import posix
import sys


def current():
    # Keep eager stdin finalizers reachable independently of the file under test.
    stdin, buffer, raw_input = sys.stdin, sys.stdin.buffer, sys.stdin.buffer.raw
    reader = open(__file__, "rb")
    reader_fd = reader.fileno()
    alias = reader
    del reader
    os.fstat(reader_fd)
    del alias
    try:
        os.fstat(reader_fd)
    except OSError as error:
        assert error.errno == 9
    else:
        raise AssertionError("reader descriptor remained open after its last reference")
    assert stdin is sys.stdin and buffer is stdin.buffer and raw_input is buffer.raw

    fd = os.open(path=__file__, flags=os.O_RDONLY)
    try:
        assert not os.get_inheritable(fd) and not os.isatty(fd)
        borrowed = open(fd, "rb", closefd=False)
        del borrowed
        os.fstat(fd)
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

    check_closerange()

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
        check_open_audit(directory)
        check_remove_audit(directory)
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


def check_closerange():
    import errno

    class Index:
        def __init__(self, value):
            self.value = value

        def __index__(self):
            return self.value

    assert vars(os)["closerange"] is os.closerange is posix.closerange
    read_fd, write_fd = os.pipe()
    try:
        assert os.closerange(read_fd, read_fd) is None
        try:
            os.closerange(read_fd, 1 << 64)
        except OverflowError:
            pass
        else:
            raise AssertionError("closerange accepted an out-of-range upper bound")
        os.fstat(read_fd)
        assert posix.closerange(Index(read_fd), Index(read_fd + 1)) is None
        try:
            os.fstat(read_fd)
        except OSError as error:
            assert error.errno == errno.EBADF
        else:
            raise AssertionError("closerange left the descriptor open")
        assert os.closerange(read_fd, read_fd + 1) is None
        os.fstat(write_fd)
    finally:
        os.close(write_fd)
        try:
            os.close(read_fd)
        except OSError as error:
            assert error.errno == errno.EBADF


def check_open_audit(directory):
    spelling = b"imre-open-audit-veto"
    flags = os.O_RDWR | os.O_CREAT | os.O_EXCL
    events = []

    class Path:
        def __fspath__(self):
            return spelling

    def hook(event, args):
        if armed and event == "open" and args[0] is spelling:
            events.append(args)
            raise PermissionError("os.open veto")

    armed = True
    sys.addaudithook(hook)
    try:
        fd = posix.open(Path(), flags, 0o600, dir_fd=directory)
    except PermissionError as error:
        assert str(error) == "os.open veto"
    else:
        os.close(fd)
        os.unlink(spelling, dir_fd=directory)
        raise AssertionError("os.open ignored its audit hook")
    finally:
        armed = False
    assert events == [(spelling, None, flags | os.O_CLOEXEC)]
    assert not os.path.lexists(spelling)


def check_remove_audit(directory):
    path = b"imre-boxed-remove-audit"
    with open(path, "xb") as file:
        file.write(b"keep")
    events = []

    def audit(event, args):
        if armed and event == "os.remove" and args[0] is path:
            events.append(args)
            raise PermissionError("veto remove")

    armed = True
    try:
        sys.addaudithook(audit)
        for remove in (os.unlink, posix.remove):
            try:
                remove(path, dir_fd=directory)
            except PermissionError as error:
                assert str(error) == "veto remove"
            else:
                raise AssertionError("audit hook did not prevent removal")
        assert events == [(path, directory), (path, directory)]
        with open(path, "rb") as file:
            assert file.read() == b"keep"
    finally:
        armed = False
        if os.path.lexists(path):
            os.unlink(path)
