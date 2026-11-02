import os
import posix
import sys
from os import chown, mkfifo, supports_dir_fd, supports_effective_ids, supports_fd, supports_follow_symlinks


def current():
    assert os.openpty is posix.openpty
    master, slave = os.openpty()
    try:
        assert os.isatty(master) and os.isatty(slave)
        assert not os.get_inheritable(master) and not os.get_inheritable(slave)
        assert os.write(slave, b"pty") == 3
        assert os.read(master, 3) == b"pty"
    finally:
        os.close(slave)
        os.close(master)
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

    assert chown is posix.chown and all(chown in table for table in capabilities[:3])
    assert os.getgroups is posix.getgroups and all(type(gid) is int for gid in os.getgroups())
    assert os.fchown is posix.fchown and os.lchown is posix.lchown
    assert os.utime is posix.utime and all(os.utime in table for table in capabilities[:3])
    assert os.chmod is posix.chmod and os.chmod in supports_fd and os.chmod in supports_dir_fd
    assert (os.chmod in supports_follow_symlinks) == (sys.platform == "darwin")
    assert os.access is posix.access
    assert os.access in supports_dir_fd and os.access in supports_effective_ids
    assert os.access in supports_follow_symlinks and os.access not in supports_fd
    assert os.access(path=__file__, mode=os.R_OK)
    for path, mode, expected in ((None, 0, TypeError), (b"name\0", 0, ValueError),
                                 (__file__, 1 << 70, OverflowError)):
        try:
            os.access(path, mode)
        except expected:
            pass
        else:
            raise AssertionError("invalid access argument accepted")
    assert mkfifo is os.mkfifo and mkfifo is posix.mkfifo
    assert mkfifo not in supports_fd and mkfifo not in supports_follow_symlinks
    if sys.platform == "linux":
        read_fd, write_fd = os.pipe2(0)
        try:
            assert os.get_inheritable(read_fd) and os.get_inheritable(write_fd)
        finally:
            os.close(write_fd)
            os.close(read_fd)
        assert mkfifo in supports_dir_fd
        from os import MFD_CLOEXEC, memfd_create
        assert memfd_create is posix.memfd_create and MFD_CLOEXEC == posix.MFD_CLOEXEC
        for flags, inherited in ((0, True), (MFD_CLOEXEC + (1 << 70), False)):
            fd = memfd_create(b"test", flags=flags)
            try:
                assert os.get_inheritable(fd) is inherited
                with open(fd, "wb", closefd=False) as stream:
                    stream.write(b"memfd")
                    assert stream.tell() == 5
            finally:
                os.close(fd)
    assert os.mkdir is posix.mkdir and os.mkdir in supports_dir_fd
    directory = os.path.dirname(__file__) or "."
    fd = os.open(directory, os.O_RDONLY)
    child = os.fsencode(os.path.basename(__file__) + ".mkdir")
    try:
        assert not os.access(child, os.F_OK, dir_fd=fd)
        assert os.mkdir(child, mode=0o700, dir_fd=fd) is None
        try:
            assert os.access(child, os.X_OK, dir_fd=fd, effective_ids=True)
            if mkfifo in supports_dir_fd:
                fifo = child + b"/fifo"
                assert mkfifo(path=fifo, mode=0o600, dir_fd=fd) is None
                try:
                    assert os.lstat(fifo, dir_fd=fd).st_mode & 0o170000 == 0o010000
                    try:
                        posix.mkfifo(fifo, dir_fd=fd)
                    except FileExistsError as error:
                        assert error.filename is None
                    else:
                        raise AssertionError("mkfifo accepted an existing FIFO")
                finally:
                    os.unlink(fifo, dir_fd=fd)
            executable = child + b"/program"
            program_fd = os.open(executable, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600, dir_fd=fd)
            try:
                os.close(program_fd)
                assert not os.access(executable, os.X_OK, dir_fd=fd)
                os.chmod(executable, 0o700, dir_fd=fd)
                assert os.access(executable, os.X_OK, dir_fd=fd)
            finally:
                os.unlink(executable, dir_fd=fd)
            dangling = child + b".dangling"
            os.symlink(dangling + b".missing", dangling, dir_fd=fd)
            try:
                assert chown(dangling, -1, -1, dir_fd=fd, follow_symlinks=False) is None
                assert not os.access(dangling, os.F_OK, dir_fd=fd)
                assert os.access(dangling, os.F_OK, dir_fd=fd, follow_symlinks=False)
            finally:
                os.unlink(dangling, dir_fd=fd)
            assert os.stat(child, dir_fd=fd).st_mode & 0o170000 == 0o040000
            assert os.chmod(path=child, mode=0o710, dir_fd=fd) is None
            child_fd = os.open(child, os.O_RDONLY, dir_fd=fd)
            try:
                assert os.fstat(child_fd).st_mode & 0o777 == 0o710
                owner = os.fstat(child_fd)
                assert chown(child, owner.st_uid, -1, dir_fd=fd) is None
                assert posix.fchown(child_fd, -1, owner.st_gid) is None
                try:
                    chown(child_fd, -1, -1, follow_symlinks=False)
                except ValueError:
                    pass
                else:
                    raise AssertionError("chown accepted a no-follow flag with an open fd")
                stamps = (1_234_567_890_123_456_789, 1_345_678_901_234_567_891)
                assert os.utime(child_fd, ns=stamps) is None
                changed = os.stat(child, dir_fd=fd)
                assert (changed.st_atime_ns, changed.st_mtime_ns) == stamps
                assert posix.chmod(child_fd, 0o700) is None
                assert os.stat(child, dir_fd=fd).st_mode & 0o777 == 0o700
            finally:
                os.close(child_fd)
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
