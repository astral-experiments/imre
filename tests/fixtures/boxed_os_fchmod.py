import errno
import os
import posix
import stat
import sys
import tempfile


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


def current():
    assert os.fchmod is posix.fchmod
    fd, path = tempfile.mkstemp()
    moved = path + ".moved"
    armed = True
    try:
        os.rename(path, moved)
        with open(path, "xb", 0) as replacement:
            os.chmod(path, 0o600)
            assert os.fchmod(mode=Index(0o640), fd=Index(fd)) is None
            original = os.fstat(fd)
            assert original.st_ino == os.stat(moved).st_ino
            assert original.st_ino != os.fstat(replacement.fileno()).st_ino
            assert stat.S_IMODE(original.st_mode) == 0o640
            assert stat.S_IMODE(os.stat(path).st_mode) == 0o600

        def veto(event, args):
            if armed and event == "os.chmod" and args == (fd, 0o400, -1):
                raise PermissionError("fchmod veto")

        sys.addaudithook(veto)
        try:
            os.fchmod(fd, 0o400)
        except PermissionError as exc:
            assert str(exc) == "fchmod veto"
        else:
            raise AssertionError("ignored audit veto")
        assert stat.S_IMODE(os.fstat(fd).st_mode) == 0o640
    finally:
        armed = False
        os.close(fd)
        os.unlink(path)
        os.unlink(moved)

    try:
        os.fchmod(fd, 0o600)
    except OSError as exc:
        assert exc.errno == errno.EBADF and exc.filename is None
    else:
        raise AssertionError("closed fd accepted")
    for file, mode, expected in ((path, 0, TypeError), (fd, 1 << 80, OverflowError)):
        try:
            os.fchmod(file, mode)
        except expected:
            pass
        else:
            raise AssertionError("invalid fchmod argument accepted")
