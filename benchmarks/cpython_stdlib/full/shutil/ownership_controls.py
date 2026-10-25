"""Compare chown results, error types, errno arguments and spelling on real descriptors."""
from contextlib import ExitStack
import os
from pathlib import Path
import shutil
import tempfile

from misc_ops import Ownership


def compare(tools, folder):
    with tempfile.TemporaryDirectory() as temp, ExitStack() as stack:
        root = Path(temp)
        name = str(root / 'one')
        open(name, 'x').close()
        os.symlink('missing', root / 'dangling')
        fd = os.open(name, os.O_RDONLY)
        stack.callback(os.close, fd)
        dir_fd = os.open(root, os.O_RDONLY)
        stack.callback(os.close, dir_fd)
        run = Ownership(tools, folder)
        uid, gid = os.getuid(), os.getgid()
        calls = [
            (fd, uid, gid, {}), (fd, uid, gid, {'follow_symlinks': False}),
            (fd, uid, gid, {'dir_fd': dir_fd}), (-2, uid, gid, {}), (-2, uid, gid, {'dir_fd': dir_fd}), (-1, uid, gid, {}),
            ('one', uid, gid, {'dir_fd': dir_fd}), ('dangling', uid, gid, {'dir_fd': dir_fd}),
            ('dangling', uid, gid, {'dir_fd': dir_fd, 'follow_symlinks': False}),
            (os.fsencode(root / 'missing'), uid, gid, {}),
            (str(root / 'missing'), uid, gid, {}), (root / 'missing', uid, gid, {}),
            (70000, uid, gid, {}),
            (name, -2, None, {}), (name, 2**32, None, {}), (name, 2**100, None, {}),
            (name, -2**100, None, {}), (name, 2**32-1, None, {}),
            (name, -1, b'invalid-group', {}), (None, 3.5, None, {}),
            (name, 2**100, 3.5, {}), (name, None, None, {'dir_fd': 'bad'}),
            (name, -1, -1, {'dir_fd': 2**100}),
        ]
        def capture(fn, p, u, g, kwargs):
            try:
                return ('return', fn(p, u, g, **kwargs))
            except (ValueError, TypeError, OverflowError, LookupError, OSError) as exc:
                return (type(exc), repr(exc), str(exc), exc.args, *(getattr(exc, a, None)
                        for a in ('errno', 'strerror', 'filename', 'filename2')))
        for p, u, g, kwargs in calls:
            expected = capture(shutil.chown, p, u, g, kwargs)
            actual = capture(run, p, u, g, kwargs)
            assert actual == expected, (p, u, g, kwargs, actual, expected)
        return len(calls)
