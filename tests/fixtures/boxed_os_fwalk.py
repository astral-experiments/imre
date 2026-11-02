import errno
import os
import shutil
import tempfile


def current():
    root = tempfile.mkdtemp()
    parent = os.open(root, os.O_RDONLY)
    try:
        os.mkdir("child", dir_fd=parent)
        os.symlink("child", "alias", dir_fd=parent)

        walk = os.fwalk(".", dir_fd=parent)
        path, dirs, files, top_fd = next(walk)
        assert path == "." and set(dirs) == {"child", "alias"} and not files
        assert os.path.samestat(os.stat(root), os.fstat(top_fd))
        dirs[:] = ["child"]
        path, dirs, files, child_fd = next(walk)
        assert path == "./child" and not dirs and not files
        assert os.path.samestat(os.stat("child", dir_fd=parent), os.fstat(child_fd))
        walk.close()

        for fd in (top_fd, child_fd):
            try:
                os.fstat(fd)
            except OSError as exc:
                assert exc.errno == errno.EBADF
            else:
                raise AssertionError("fwalk retained a directory descriptor after close()")

        assert list(os.fwalk("alias", dir_fd=parent)) == []
        assert [path for path, _, _, _ in os.fwalk(b"alias", follow_symlinks=True, dir_fd=parent)] == [b"alias"]
    finally:
        os.close(parent)
        shutil.rmtree(root)
