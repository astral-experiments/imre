"""POSIX named temporary files and directories for runtime-loaded modules."""

import functools as _functools
import io as _io
import sys as _sys
import warnings as _warnings
import os as _os
import errno as _errno
from _thread import allocate_lock as _allocate_lock

from _imre_tempfile import abspath as _abspath
from _imre_tempfile import finalize_at_shutdown as _finalize_at_shutdown
from _imre_tempfile import mkdir as _mkdir, random_name as _random_name

TMP_MAX = 20
template = "tmp"
tempdir = None
_text_openflags = _os.O_RDWR | _os.O_CREAT | _os.O_EXCL | _os.O_NOFOLLOW
_bin_openflags = _text_openflags


class _RandomNameSequence:
    def __iter__(self):
        return self

    def __next__(self):
        return _random_name()


_name_sequence = _RandomNameSequence()
_once_lock = _allocate_lock()


def _get_candidate_names():
    return _name_sequence


# From CPython 3.14.5 Lib/tempfile.py. See ../docstrings/LICENSE.
def _exists(fn):
    try:
        _os.lstat(fn)
    except OSError:
        return False
    else:
        return True


def _fsencode(path):
    path = _os.fspath(path)
    if isinstance(path, str):
        return path.encode("utf-8", "surrogateescape")
    return path


# From CPython 3.14.5 Lib/tempfile.py. See ../docstrings/LICENSE.
def _candidate_tempdir_list():
    """Generate a list of candidate temporary directories which
    _get_default_tempdir will try."""

    dirlist = []

    # First, try the environment.
    for envname in 'TMPDIR', 'TEMP', 'TMP':
        dirname = _os.getenv(envname)
        if dirname: dirlist.append(dirname)

    # Failing that, try OS-specific locations.
    if _os.name == 'nt':
        dirlist.extend([ _os.path.expanduser(r'~\AppData\Local\Temp'),
                         _os.path.expandvars(r'%SYSTEMROOT%\Temp'),
                         r'c:\temp', r'c:\tmp', r'\temp', r'\tmp' ])
    else:
        dirlist.extend([ '/tmp', '/var/tmp', '/usr/tmp' ])

    # As a last resort, the current directory.
    try:
        dirlist.append(_os.getcwd())
    except (AttributeError, OSError):
        dirlist.append(_os.curdir)

    return dirlist


def _get_default_tempdir(dirlist=None):
    """Calculate the default directory to use for temporary files.
    This routine should be called exactly once.

    We determine whether or not a candidate temp dir is usable by
    trying to create and write to a file in that directory.  If this
    is successful, the test file is deleted.  To prevent denial of
    service, the name of the test file must be randomized."""

    namer = _RandomNameSequence()
    if dirlist is None:
        dirlist = _candidate_tempdir_list()

    for dir in dirlist:
        if dir != _os.curdir:
            dir = _os.path.abspath(dir)
        for seq in range(TMP_MAX):
            name = next(namer)
            filename = _os.path.join(dir, name)
            try:
                fd = _os.open(filename, _bin_openflags, 0o600)
                try:
                    try:
                        _os.write(fd, b'blat')
                    finally:
                        _os.close(fd)
                finally:
                    _os.unlink(filename)
                return dir
            except FileExistsError:
                pass
            except PermissionError:
                # See the comment in mkdtemp().
                if _os.name == 'nt' and _os.path.isdir(dir):
                    continue
                break   # no point trying more names in this directory
            except OSError:
                break   # no point trying more names in this directory
    raise FileNotFoundError(_errno.ENOENT,
                            "No usable temporary directory found in %s" %
                            dirlist)


def _gettempdir():
    """Private accessor for tempfile.tempdir."""
    global tempdir
    if tempdir is None:
        _once_lock.acquire()
        try:
            if tempdir is None:
                tempdir = _get_default_tempdir()
        finally:
            _once_lock.release()
    return tempdir


def gettempdir():
    return _os.fsdecode(_gettempdir())


def gettempdirb():
    return _os.fsencode(_gettempdir())


def gettempprefix():
    return _os.fsdecode(template)


def gettempprefixb():
    return _os.fsencode(template)


# From CPython 3.14.5 Lib/tempfile.py. See ../docstrings/LICENSE.
def _infer_return_type(*args):
    return_type = None
    for arg in args:
        if arg is None:
            continue
        if isinstance(arg, _os.PathLike):
            arg = _os.fspath(arg)
        if isinstance(arg, bytes):
            if return_type is str:
                raise TypeError("Can't mix bytes and non-bytes in path components.")
            return_type = bytes
        else:
            if return_type is bytes:
                raise TypeError("Can't mix bytes and non-bytes in path components.")
            return_type = str
    if return_type is None:
        return str if tempdir is None or isinstance(tempdir, str) else bytes
    return return_type


def _candidate_path(directory, prefix, suffix, name, output_type):
    directory = _os.fspath(directory)
    if output_type is bytes:
        name = _fsencode(name)
    leaf = prefix + name + suffix
    sep = b"/" if isinstance(directory, bytes) else "/"
    if isinstance(directory, bytes) != isinstance(leaf, bytes):
        raise TypeError("Can't mix strings and bytes in path components")
    if leaf[:1] == sep or not directory:
        return leaf
    if directory[-1:] == sep:
        return directory + leaf
    return directory + sep + leaf


def _mkstemp_inner(dir, pre, suf, flags, output_type):
    def open_candidate(path):
        return _os.open(path, flags, 0o600)

    return _create_file(dir, pre, suf, output_type, open_candidate)


def _create_file(dir, pre, suf, output_type, create, *args):
    dir = _abspath(dir)
    names = _get_candidate_names()
    for _ in range(TMP_MAX):
        path = _candidate_path(dir, pre, suf, next(names), output_type)
        _sys.audit("tempfile.mkstemp", path)
        try:
            result = create(path, *args)
        except FileExistsError:
            continue
        return result, path
    raise FileExistsError(_errno.EEXIST, "No usable temporary file name found")


def _sanitize_params(prefix, suffix, dir):
    output_type = _infer_return_type(prefix, suffix, dir)
    if suffix is None:
        suffix = output_type()
    if prefix is None:
        prefix = template if output_type is str else _fsencode(template)
    if dir is None:
        dir = gettempdir() if output_type is str else gettempdirb()
    return prefix, suffix, dir, output_type


def mkstemp(suffix=None, prefix=None, dir=None, text=False):
    prefix, suffix, dir, output_type = _sanitize_params(prefix, suffix, dir)
    flags = _text_openflags if text else _bin_openflags
    return _mkstemp_inner(dir, prefix, suffix, flags, output_type)


def mkdtemp(suffix=None, prefix=None, dir=None):
    prefix, suffix, dir, output_type = _sanitize_params(prefix, suffix, dir)
    names = _get_candidate_names()
    for _ in range(TMP_MAX):
        path = _candidate_path(dir, prefix, suffix, next(names), output_type)
        try:
            _mkdir(path)
        except FileExistsError:
            continue
        return _abspath(path)
    raise FileExistsError(_errno.EEXIST, "No usable temporary directory name found")


# From CPython 3.14.5 Lib/tempfile.py. See ../docstrings/LICENSE.
def mktemp(suffix="", prefix=template, dir=None):
    """User-callable function to return a unique temporary file name.  The
    file is not created.

    Arguments are similar to mkstemp, except that the 'text' argument is
    not accepted, and suffix=None, prefix=None and bytes file names are not
    supported.

    THIS FUNCTION IS UNSAFE AND SHOULD NOT BE USED.  The file name may
    refer to a file that did not exist at some point, but by the time
    you get around to creating it, someone else may have beaten you to
    the punch.
    """

##    from warnings import warn as _warn
##    _warn("mktemp is a potential security risk to your program",
##          RuntimeWarning, stacklevel=2)

    if dir is None:
        dir = gettempdir()

    names = _get_candidate_names()
    for seq in range(TMP_MAX):
        name = next(names)
        file = _os.path.join(dir, prefix + name + suffix)
        if not _exists(file):
            return file

    raise FileExistsError(_errno.EEXIST,
                          "No usable temporary filename found")


# From CPython 3.14.5 Lib/tempfile.py. See ../docstrings/LICENSE.
@_finalize_at_shutdown
class _TemporaryFileCloser:
    """A separate object allowing proper closing of a temporary file's
    underlying file object, without adding a __del__ method to the
    temporary file."""

    cleanup_called = False
    close_called = False

    def __init__(
        self,
        file,
        name,
        delete=True,
        delete_on_close=True,
        warn_message="Implicitly cleaning up unknown file",
    ):
        self.file = file
        self.name = name
        self.delete = delete
        self.delete_on_close = delete_on_close
        self.warn_message = warn_message

    def cleanup(self, windows=(_os.name == 'nt'), unlink=_os.unlink):
        if not self.cleanup_called:
            self.cleanup_called = True
            try:
                if not self.close_called:
                    self.close_called = True
                    self.file.close()
            finally:
                # Windows provides delete-on-close as a primitive, in which
                # case the file was deleted by self.file.close().
                if self.delete and not (windows and self.delete_on_close):
                    try:
                        unlink(self.name)
                    except FileNotFoundError:
                        pass

    def close(self):
        if not self.close_called:
            self.close_called = True
            try:
                self.file.close()
            finally:
                if self.delete and self.delete_on_close:
                    self.cleanup()

    def __del__(self):
        close_called = self.close_called
        self.cleanup()
        if not close_called:
            _warnings.warn(self.warn_message, ResourceWarning)


class _TemporaryFileWrapper:
    """Temporary file wrapper

    This class provides a wrapper around files opened for
    temporary use.  In particular, it seeks to automatically
    remove the file when it is no longer needed.
    """

    def __init__(self, file, name, delete=True, delete_on_close=True):
        self.file = file
        self.name = name
        self._closer = _TemporaryFileCloser(
            file,
            name,
            delete,
            delete_on_close,
            warn_message=f"Implicitly cleaning up {self!r}",
        )

    def __repr__(self):
        file = self.__dict__['file']
        return f"<{type(self).__name__} {file=}>"

    def __getattr__(self, name):
        # Attribute lookups are delegated to the underlying file
        # and cached for non-numeric results
        # (i.e. methods are cached, closed and friends are not)
        file = self.__dict__['file']
        a = getattr(file, name)
        # Some native methods are callable without exposing their __call__ slot.
        if hasattr(a, '__call__') or isinstance(a, type(len)):
            func = a
            @_functools.wraps(func)
            def func_wrapper(*args, **kwargs):
                return func(*args, **kwargs)
            # Avoid closing the file as long as the wrapper is alive,
            # see issue #18879.
            func_wrapper._closer = self._closer
            a = func_wrapper
        if not isinstance(a, int):
            setattr(self, name, a)
        return a

    # The underlying __enter__ method returns the wrong object
    # (self.file) so override it to return the wrapper
    def __enter__(self):
        self.file.__enter__()
        return self

    # Need to trap __exit__ as well to ensure the file gets
    # deleted when used in a with statement
    def __exit__(self, exc, value, tb):
        result = self.file.__exit__(exc, value, tb)
        self._closer.cleanup()
        return result

    def close(self):
        """
        Close the temporary file, possibly deleting it.
        """
        self._closer.close()

    # iter() doesn't use __getattr__ to find the __iter__ method
    def __iter__(self):
        # Don't return iter(self.file), but yield from it to avoid closing
        # file as long as it's being used as iterator (see issue #23700).  We
        # can't use 'yield from' here because iter(file) returns the file
        # object itself, which has a close method, and thus the file would get
        # closed when the generator is finalized, due to PEP380 semantics.
        for line in self.file:
            yield line

# From CPython 3.14.5 Lib/tempfile.py. See ../docstrings/LICENSE.
def NamedTemporaryFile(mode="w+b", buffering=-1, encoding=None, newline=None,
                       suffix=None, prefix=None, dir=None, delete=True, *,
                       errors=None, delete_on_close=True):
    prefix, suffix, dir, output_type = _sanitize_params(prefix, suffix, dir)

    flags = _bin_openflags

    # Setting O_TEMPORARY in the flags causes the OS to delete
    # the file when it is closed.  This is only supported by Windows.
    if _os.name == 'nt' and delete and delete_on_close:
        flags |= _os.O_TEMPORARY

    if "b" not in mode:
        encoding = _io.text_encoding(encoding)

    name = None
    def opener(*args):
        nonlocal name
        fd, name = _mkstemp_inner(dir, prefix, suffix, flags, output_type)
        return fd
    try:
        file = _io.open(dir, mode, buffering=buffering,
                        newline=newline, encoding=encoding, errors=errors,
                        opener=opener)
        try:
            raw = getattr(file, 'buffer', file)
            raw = getattr(raw, 'raw', raw)
            raw.name = name
            return _TemporaryFileWrapper(file, name, delete, delete_on_close)
        except:
            file.close()
            raise
    except:
        if name is not None and not (
            _os.name == 'nt' and delete and delete_on_close):
            _os.unlink(name)
        raise


# From CPython 3.14.5 Lib/tempfile.py. See ../docstrings/LICENSE.
if _os.name != 'posix' or _sys.platform == 'cygwin':
    # On non-POSIX and Cygwin systems, assume that we cannot unlink a file
    # while it is open.
    TemporaryFile = NamedTemporaryFile

else:
    # Is the O_TMPFILE flag available and does it work?
    # The flag is set to False if os.open(dir, os.O_TMPFILE) raises an
    # IsADirectoryError exception
    _O_TMPFILE_WORKS = hasattr(_os, 'O_TMPFILE')

    def TemporaryFile(mode='w+b', buffering=-1, encoding=None,
                      newline=None, suffix=None, prefix=None,
                      dir=None, *, errors=None):
        """Create and return a temporary file.
        Arguments:
        'prefix', 'suffix', 'dir' -- as for mkstemp.
        'mode' -- the mode argument to io.open (default "w+b").
        'buffering' -- the buffer size argument to io.open (default -1).
        'encoding' -- the encoding argument to io.open (default None)
        'newline' -- the newline argument to io.open (default None)
        'errors' -- the errors argument to io.open (default None)
        The file is created as mkstemp() would do it.

        Returns an object with a file-like interface.  The file has no
        name, and will cease to exist when it is closed.
        """
        global _O_TMPFILE_WORKS

        if "b" not in mode:
            encoding = _io.text_encoding(encoding)

        prefix, suffix, dir, output_type = _sanitize_params(prefix, suffix, dir)

        flags = _bin_openflags
        if _O_TMPFILE_WORKS:
            fd = None
            def opener(*args):
                nonlocal fd
                flags2 = (flags | _os.O_TMPFILE) & ~_os.O_CREAT
                fd = _os.open(dir, flags2, 0o600)
                return fd
            try:
                file = _io.open(dir, mode, buffering=buffering,
                                newline=newline, encoding=encoding,
                                errors=errors, opener=opener)
                raw = getattr(file, 'buffer', file)
                raw = getattr(raw, 'raw', raw)
                raw.name = fd
                return file
            except IsADirectoryError:
                # Linux kernel older than 3.11 ignores the O_TMPFILE flag:
                # O_TMPFILE is read as O_DIRECTORY. Trying to open a directory
                # with O_RDWR|O_DIRECTORY fails with IsADirectoryError, a
                # directory cannot be open to write. Set flag to False to not
                # try again.
                _O_TMPFILE_WORKS = False
            except OSError:
                # The filesystem of the directory does not support O_TMPFILE.
                # For example, OSError(95, 'Operation not supported').
                #
                # On Linux kernel older than 3.11, trying to open a regular
                # file (or a symbolic link to a regular file) with O_TMPFILE
                # fails with NotADirectoryError, because O_TMPFILE is read as
                # O_DIRECTORY.
                pass
            # Fallback to _mkstemp_inner().

        fd = None
        def opener(*args):
            nonlocal fd
            fd, name = _mkstemp_inner(dir, prefix, suffix, flags, output_type)
            try:
                _os.unlink(name)
            except BaseException as e:
                _os.close(fd)
                raise
            return fd
        file = _io.open(dir, mode, buffering=buffering,
                        newline=newline, encoding=encoding, errors=errors,
                        opener=opener)
        raw = getattr(file, 'buffer', file)
        raw = getattr(raw, 'raw', raw)
        raw.name = fd
        return file
