"""Filesystem predicates shared by the POSIX path modules."""
from _imre_posixpath import exists as _exists, isfile as _isfile, islink as _islink


def exists(path):
    try:
        return _exists(path)
    except (OSError, ValueError):
        return False


def isfile(path):
    try:
        return _isfile(path)
    except (OSError, ValueError):
        return False


def islink(path):
    try:
        return _islink(path)
    except (OSError, ValueError, AttributeError):
        return False


# From CPython 3.14.5, Lib/genericpath.py. See stdlib/docstrings/LICENSE.
def commonprefix(m):
    "Given a list of pathnames, returns the longest common leading component"
    if not m: return ''
    # Some people pass in a list of pathname parts to operate in an OS-agnostic
    # fashion; don't try to translate in that case as that's an abuse of the
    # API and they are already doing what they need to be OS-agnostic and so
    # they most likely won't be using an os.PathLike object in the sublists.
    if not isinstance(m[0], (list, tuple)):
        m = tuple(map(os.fspath, m))
    s1 = min(m)
    s2 = max(m)
    for i, c in enumerate(s1):
        if c != s2[i]:
            return s1[:i]
    return s1


# os imports posixpath, which re-exports the functions above.
import os
