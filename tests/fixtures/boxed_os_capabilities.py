import os
import posix
from os import supports_dir_fd, supports_effective_ids, supports_fd, supports_follow_symlinks


def current():
    assert os.listdir in os.supports_fd
    capabilities = (supports_dir_fd, supports_fd, supports_follow_symlinks, supports_effective_ids)
    operations = (os.lstat, os.open, os.rmdir, os.stat, os.unlink, os.remove, os.fstat)
    expected = ((True, True, True, True, True, False, False),
                (False, False, False, True, False, False, False),
                (False, False, False, True, False, False, False),
                (False, False, False, False, False, False, False))
    for functions, row in zip(capabilities, expected):
        assert type(functions) is set
        for function, member in zip(operations, row):
            assert getattr(posix, function.__name__) is function
            assert (function in functions) == member

    assert os.mkdir is posix.mkdir and os.mkdir in supports_dir_fd
    directory = os.path.dirname(__file__) or "."
    fd = os.open(directory, os.O_RDONLY)
    child = os.fsencode(os.path.basename(__file__) + ".mkdir")
    try:
        assert os.mkdir(child, mode=0o700, dir_fd=fd) is None
        try:
            assert os.stat(child, dir_fd=fd).st_mode & 0o170000 == 0o040000
            try:
                posix.mkdir(child, dir_fd=fd)
            except FileExistsError as error:
                assert error.filename == child
            else:
                raise AssertionError("mkdir accepted an existing directory")
        finally:
            os.rmdir(child, dir_fd=fd)
    finally:
        os.close(fd)

    original = os.stat
    try:
        os.stat = None
        assert original in supports_fd and os.stat not in supports_fd
        supports_fd.add(None)
        assert None in os.supports_fd and None not in supports_follow_symlinks
        os.supports_fd = set()
        from os import supports_fd as rebound
        assert rebound is os.supports_fd and rebound is not supports_fd and not rebound
        assert None in supports_fd
        del os.supports_fd
        assert not hasattr(os, "supports_fd")
    finally:
        os.stat = original
        supports_fd.discard(None)
        os.supports_fd = supports_fd
    assert os.__dict__["supports_fd"] is supports_fd
    assert not hasattr(posix, "supports_fd")
    print("capabilities checked")
