import errno
import os
import posix
import subprocess
import sys
from os import chdir


class Directory:
    def __init__(self, name):
        self.name = name

    def __fspath__(self):
        return self.name


def current():
    assert chdir is os.chdir is posix.chdir
    assert chdir in os.supports_fd
    original = os.getcwd()
    descriptor = os.open(".", os.O_RDONLY)
    dirname = "imre-chdir-child"
    missing = b"imre-chdir-missing"
    events = []
    def deny(event, args):
        if event == "os.chdir":
            events.append(args[0])
        if event == "os.chdir" and args == (b"imre-audit-denied",):
            raise RuntimeError("chdir audit denied")

    sys.addaudithook(deny)
    os.mkdir(dirname)
    try:
        assert chdir(path=Directory(dirname.encode())) is None
        assert os.getcwd() == os.path.join(original, dirname)
        assert chdir(descriptor) is None
        assert os.getcwd() == original
        chdir(dirname)
        try:
            chdir(Directory(missing))
        except FileNotFoundError as error:
            assert error.errno == errno.ENOENT and error.filename is missing
        else:
            raise AssertionError("changed into a nonexistent directory")
        child_cwd = subprocess.check_output(["/bin/pwd", "-P"])
        assert child_cwd == os.fsencode(os.path.join(original, dirname)) + b"\n"
        chdir("..")
        fd = os.open(__file__, os.O_RDONLY)
        try:
            try:
                chdir(fd)
            except NotADirectoryError as error:
                assert error.errno == errno.ENOTDIR and error.filename == fd
            else:
                raise AssertionError("changed into a regular file")
        finally:
            os.close(fd)
        class ClosedDescriptor:
            def __index__(self):
                return -83

        for value, code in ((ClosedDescriptor(), errno.EBADF), (-1, errno.EFAULT)):
            try:
                chdir(value)
            except OSError as error:
                assert error.errno == code and error.filename is value
                assert events[-1] is value
            else:
                raise AssertionError("accepted an invalid directory descriptor")
        try:
            chdir(Directory(b"imre-audit-denied"))
        except RuntimeError as error:
            assert str(error) == "chdir audit denied"
        else:
            raise AssertionError("ignored a failing audit hook")
        assert os.getcwd() == original
    finally:
        chdir(descriptor)
        os.close(descriptor)
        os.rmdir(dirname)
