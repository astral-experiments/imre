# File URL conversion from CPython 3.14.5 Lib/urllib/request.py (PSF-2.0).
import os
import sys
from urllib.parse import quote

__all__ = ["pathname2url"]


def pathname2url(pathname, *, add_scheme=False):
    """Convert the given local file system path to a file URL.

    The 'file:' scheme prefix is omitted unless *add_scheme*
    is set to true.
    """
    if os.name == 'nt':
        pathname = pathname.replace('\\', '/')
    encoding = sys.getfilesystemencoding()
    errors = sys.getfilesystemencodeerrors()
    scheme = 'file:' if add_scheme else ''
    drive, root, tail = os.path.splitroot(pathname)
    if drive:
        # First, clean up some special forms. We are going to sacrifice the
        # additional information anyway
        if drive[:4] == '//?/':
            drive = drive[4:]
            if drive[:4].upper() == 'UNC/':
                drive = '//' + drive[4:]
        if drive[1:] == ':':
            # DOS drive specified. Add three slashes to the start, producing
            # an authority section with a zero-length authority, and a path
            # section starting with a single slash.
            drive = '///' + drive
        drive = quote(drive, encoding=encoding, errors=errors, safe='/:')
    elif root:
        # Add explicitly empty authority to absolute path. If the path
        # starts with exactly one slash then this change is mostly
        # cosmetic, but if it begins with two or more slashes then this
        # avoids interpreting the path as a URL authority.
        root = '//' + root
    tail = quote(tail, encoding=encoding, errors=errors)
    return scheme + drive + root + tail
