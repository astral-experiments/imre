"""Filesystem predicates shared by the POSIX path modules."""
from _imre_posixpath import exists as _exists, isdir as _isdir, isfile as _isfile, islink as _islink


def exists(path):
    try:
        return _exists(path)
    except (OSError, ValueError):
        return False


def isdir(path):
    try:
        return _isdir(path)
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
def getsize(filename):
    """Return the size of a file, reported by os.stat()."""
    return os.stat(filename).st_size


def samestat(s1, s2):
    """Test whether two stat buffers reference the same file"""
    return (s1.st_ino == s2.st_ino and
            s1.st_dev == s2.st_dev)

def samefile(f1, f2):
    """Test whether two pathnames reference the same actual file or directory

    This is determined by the device number and i-node number and
    raises an exception if an os.stat() call on either pathname fails.
    """
    s1 = os.stat(f1)
    s2 = os.stat(f2)
    return samestat(s1, s2)

def sameopenfile(fp1, fp2):
    """Test whether two open file objects reference the same file"""
    s1 = os.fstat(fp1)
    s2 = os.fstat(fp2)
    return samestat(s1, s2)


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


@object.__new__
class ALLOW_MISSING:
    """Special value for use in realpath()."""
    def __repr__(self):
        return 'os.path.ALLOW_MISSING'
    def __reduce__(self):
        return self.__class__.__name__


# os imports posixpath, which re-exports the functions above.
import os
