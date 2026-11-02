"""POSIX lexical operations shared by os.path and posixpath."""
# os reads these constants while the two modules import each other.
curdir = "."
extsep = "."

import os
import stat
from _imre_posixpath import (
    extension_start, relative_parts as _relative_parts, realpath_loop as _realpath_loop,
    realpath_parent as _realpath_parent, realpath_parts as _realpath_parts,
    realpath_kind as _realpath_kind, realpath_readlink as _realpath_readlink,
    realpath_notdir as _realpath_notdir,
)
from genericpath import (
    ALLOW_MISSING, _check_arg_types, commonprefix, exists, getatime, getctime, getmtime, getsize, isdir, isfile,
    isjunction, islink, lexists, samefile, sameopenfile, samestat,
)


# From CPython 3.14.5 Lib/posixpath.py. See stdlib/docstrings/LICENSE.
def join(a, *p):
    """Join two or more pathname components, inserting '/' as needed.
    If any component is an absolute path, all previous path components
    will be discarded.  An empty last part will result in a path that
    ends with a separator."""
    a = os.fspath(a)
    sep = b"/" if isinstance(a, bytes) else "/"
    path = a
    try:
        for b in p:
            b = os.fspath(b)
            if b.startswith(sep) or not path:
                path = b
            elif path.endswith(sep):
                path += b
            else:
                path += sep + b
    except (TypeError, AttributeError, BytesWarning):
        _check_arg_types('join', a, *p)
        raise
    return path


def split(p):
    """Split a pathname.  Returns tuple "(head, tail)" where "tail" is
    everything after the final slash.  Either part may be empty."""
    p = os.fspath(p)
    sep = b"/" if isinstance(p, bytes) else "/"
    i = p.rfind(sep) + 1
    head, tail = p[:i], p[i:]
    if head and head != sep * len(head):
        head = head.rstrip(sep)
    return head, tail


def normcase(s):
    return os.fspath(s)


def isabs(s):
    """Test whether a path is absolute."""
    return _starts_with_sep(os.fspath(s))


def _starts_with_sep(s):
    if type(s) is str:
        # A filesystem string can contain surrogate-escaped bytes.
        return s[:1] == "/"
    return s.startswith(b"/" if isinstance(s, bytes) else "/")


def splitdrive(p):
    p = os.fspath(p)
    return p[:0], p


def basename(p):
    p = os.fspath(p)
    sep = b"/" if isinstance(p, bytes) else "/"
    return p[p.rfind(sep) + 1:]


def splitext(p):
    p = os.fspath(p)
    if type(p) is str or type(p) is bytes:
        cut = extension_start(p)
        if cut == len(p):
            return p, p[:0]
        return p[:cut], p[cut:]

    # Subclass overrides participate in the searches and slices in genericpath._splitext.
    if isinstance(p, bytes):
        sep, dot = b"/", b"."
    else:
        sep, dot = "/", "."
    sep_index = p.rfind(sep)
    cut = p.rfind(dot)
    if cut > sep_index:
        start = sep_index + 1
        while start < cut:
            if p[start:start + 1] != dot:
                return p[:cut], p[cut:]
            start += 1
    return p, p[:0]


def ismount(path):
    """Test whether a path is a mount point"""
    try:
        s1 = os.lstat(path)
    except (OSError, ValueError):
        # It doesn't exist -- so not a mount point. :-)
        return False
    else:
        # A symlink can never be a mount point
        if stat.S_ISLNK(s1.st_mode):
            return False

    path = os.fspath(path)
    if isinstance(path, bytes):
        parent = join(path, b'..')
    else:
        parent = join(path, '..')
    try:
        s2 = os.lstat(parent)
    except OSError:
        parent = realpath(parent)
        try:
            s2 = os.lstat(parent)
        except OSError:
            return False

    # path/.. on a different device as path or the same i-node as path
    return s1.st_dev != s2.st_dev or s1.st_ino == s2.st_ino

def abspath(path):
    """Return a normalized absolute path."""
    path = os.fspath(path)
    if not _starts_with_sep(path):
        use_bytes = isinstance(path, bytes)
        cwd = os.getcwdb() if use_bytes else os.getcwd()
        sep = b"/" if use_bytes else "/"
        # The second startswith is the one performed by join(cwd, path).
        if not _starts_with_sep(path):
            path = cwd + path if cwd[-1:] == sep else cwd + (sep + path)
    return os.path.normpath(path)


def relpath(path, start=None):
    """Return a relative version of a path."""
    path = os.fspath(path)
    if not path:
        raise ValueError("no path specified")
    use_bytes = isinstance(path, bytes)
    if start is None:
        start = b"." if use_bytes else "."
    else:
        start = os.fspath(start)
    start = abspath(start)
    if isinstance(start, bytes) != use_bytes:
        raise TypeError("Can't mix strings and bytes in path components")
    return _relative_parts(abspath(path), start)


# Component traversal and symlink cache follow CPython 3.14.5 Lib/posixpath.py.
def realpath(filename, *, strict=False):
    """Return the canonical path, eliminating symbolic links."""
    filename = os.fspath(filename)
    if isinstance(filename, bytes):
        sep, curdir, pardir = b'/', b'.', b'..'
        getcwd = os.getcwdb
    else:
        sep, curdir, pardir = '/', '.', '..'
        getcwd = os.getcwd
    if strict is ALLOW_MISSING:
        ignored_error = FileNotFoundError
        strict = True
    elif strict:
        ignored_error = ()
    else:
        ignored_error = OSError
    rest = _realpath_parts(filename)[::-1] if type(filename) in (str, bytes) else filename.split(sep)[::-1]
    remaining = len(rest)
    path = sep if isabs(filename) else getcwd()
    seen = {}
    while remaining:
        name = rest.pop()
        if name is None:
            seen[rest.pop()] = path
            continue
        remaining -= 1
        if not name or name == curdir:
            continue
        if name == pardir:
            path = _realpath_parent(path)
            continue
        newpath = path + name if path == sep else path + sep + name
        try:
            kind = _realpath_kind(newpath)
            if kind != "link":
                if strict and remaining and kind != "dir":
                    _realpath_notdir(newpath)
                path = newpath
                continue
            if newpath in seen:
                cached = seen[newpath]
                if cached is not None:
                    path = cached
                    continue
                if strict:
                    _realpath_loop(newpath)
                path = newpath
                continue
            target = _realpath_readlink(newpath)
        except ignored_error:
            path = newpath
            continue
        if isabs(target):
            path = sep
        seen[newpath] = None
        rest.append(newpath)
        rest.append(None)
        parts = _realpath_parts(target)
        rest.extend(parts[::-1])
        remaining += len(parts)
    return path
