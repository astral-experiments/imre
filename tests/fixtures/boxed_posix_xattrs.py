import errno
import os
import posix
import tempfile


def current():
    assert os.getxattr is posix.getxattr and os.setxattr is posix.setxattr
    assert os.removexattr is posix.removexattr and os.listxattr is posix.listxattr
    try:
        os.listxattr(3.5)
    except TypeError as exc:
        assert "string, bytes, os.PathLike, integer or None" in str(exc)
    else:
        raise AssertionError("listxattr accepted a float path")
    with tempfile.TemporaryDirectory() as root:
        path = os.path.join(root, "node")
        link = os.path.join(root, "link")
        with open(path, "xb", 0) as file:
            fd = file.fileno()
            attr = b"user.test\xff"
            value = bytearray(b"v" * 513)
            try:
                os.setxattr(fd, attr, memoryview(value), os.XATTR_CREATE)
            except OSError as exc:
                if exc.errno != errno.ENOTSUP:
                    raise
                return
            os.symlink(path, link)
            assert os.getxattr(link, attr) == value
            assert attr in map(os.fsencode, os.listxattr(fd))
            assert attr not in map(os.fsencode, os.listxattr(link, follow_symlinks=False))
            try:
                os.setxattr(path, attr, b"changed", os.XATTR_CREATE)
            except FileExistsError as exc:
                assert exc.filename == path
            else:
                raise AssertionError("XATTR_CREATE replaced an existing attribute")
            assert os.getxattr(fd, attr) == value
            os.unlink(path)
            os.setxattr(fd, attr, b"", os.XATTR_REPLACE)
            assert os.getxattr(fd, attr) == b""
            os.removexattr(fd, attr)
            assert attr not in map(os.fsencode, os.listxattr(fd))
