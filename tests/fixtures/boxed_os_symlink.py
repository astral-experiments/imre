import os
import sys
from os import link, symlink
from posix import link as posix_link, symlink as posix_symlink


def check_hard_links(target, symbolic, fd):
    assert link is posix_link and link is os.link
    assert link in os.supports_dir_fd and link in os.supports_follow_symlinks
    alias = b"imre-hard-link"
    events = []

    def hook(event, args):
        if event == "os.link":
            events.append(args)
            if args[1] == "veto-hard-link":
                raise PermissionError("hard link veto")

    sys.addaudithook(hook)
    try:
        link(symbolic, alias, src_dir_fd=fd, dst_dir_fd=fd)
    except FileNotFoundError as error:
        assert error.filename is symbolic and error.filename2 is alias
    else:
        raise AssertionError("link followed a dangling source successfully")
    assert events == [(symbolic, alias, fd, fd)]

    os.close(os.open(target, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600, dir_fd=fd))
    try:
        assert link(symbolic, alias, src_dir_fd=fd, dst_dir_fd=fd) is None
        try:
            assert os.stat(alias, dir_fd=fd).st_ino == os.stat(target, dir_fd=fd).st_ino
            assert os.stat(target, dir_fd=fd).st_nlink == 2
        finally:
            os.unlink(alias, dir_fd=fd)
        assert posix_link(src=symbolic, dst=alias, follow_symlinks=False) is None
        try:
            assert os.readlink(alias) == target
            assert os.lstat(symbolic).st_ino == os.lstat(alias).st_ino
            assert os.lstat(alias).st_nlink == 2
            try:
                link(target, alias)
            except FileExistsError as error:
                assert error.filename is target and error.filename2 is alias
            else:
                raise AssertionError("link replaced an existing entry")
        finally:
            os.unlink(alias)
        try:
            link(target, "veto-hard-link")
        except PermissionError as error:
            assert str(error) == "hard link veto"
        else:
            raise AssertionError("audit hook did not veto link")
        assert events[-1] == (target, "veto-hard-link", -1, -1)
        assert not os.path.exists("veto-hard-link")
    finally:
        os.unlink(target, dir_fd=fd)


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
            check_hard_links(src, dst, fd)
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
