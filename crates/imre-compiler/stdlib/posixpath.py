"""POSIX lexical operations shared by os.path and posixpath."""
import os
from _imre_posixpath import extension_start, isfile as _isfile


def isfile(path):
    try:
        return _isfile(path)
    except (OSError, ValueError):
        return False


def normcase(s):
    return os.fspath(s)


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
    start = p.rfind(sep) + 1
    cut = p.rfind(dot)
    while start < cut:
        if p[start:start + 1] != dot:
            return p[:cut], p[cut:]
        start += 1
    return p, p[:0]
