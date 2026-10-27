"""POSIX filesystem spellings for runtime-loaded modules."""
import posixpath as path
from posix import fspath


def fsdecode(filename):
    filename = fspath(filename)
    if isinstance(filename, bytes):
        return filename.decode("utf-8", "surrogateescape")
    return filename
