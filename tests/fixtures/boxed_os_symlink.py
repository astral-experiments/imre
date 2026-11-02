import os
import sys
from os import symlink
from posix import symlink as posix_symlink


def current():
    assert symlink is posix_symlink and symlink in os.supports_dir_fd
    src = b"missing-\xff"
    dst = os.fsdecode(b"imre-link-\xfe")
    events = []

    def hook(event, args):
        if event == "os.symlink":
            events.append(args)
            if args[1] == "vetoed":
                raise PermissionError("link veto")

    sys.addaudithook(hook)
    try:
        symlink(src, dst, src=src, dir_f=3)
    except TypeError as error:
        assert str(error) == "argument for symlink() given by name ('src') and position (1)"
    else:
        raise AssertionError("duplicate src accepted")
    assert not events
    fd = os.open(".", os.O_RDONLY)
    try:
        assert symlink(src, dst, dir_fd=fd) is None
        try:
            assert os.path.islink(dst)
            assert os.readlink(os.fsencode(dst), dir_fd=fd) == src
            assert events == [(src, dst, fd)]
            try:
                posix_symlink(src=src, dst=dst, target_is_directory=True)
            except FileExistsError as error:
                assert error.filename is src and error.filename2 is dst
            else:
                raise AssertionError("symlink replaced an existing entry")
        finally:
            os.unlink(dst, dir_fd=fd)
    finally:
        os.close(fd)
    try:
        symlink(src, "vetoed")
    except PermissionError as error:
        assert str(error) == "link veto"
    else:
        raise AssertionError("audit hook did not veto symlink")
    assert events[-1] == (src, "vetoed", -1) and not os.path.islink("vetoed")
