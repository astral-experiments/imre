"""POSIX named temporary files and directories for runtime-loaded modules."""

import functools as _functools
import warnings as _warnings
import os as _os
import errno as _errno
from _thread import allocate_lock as _allocate_lock

from _imre_tempfile import abspath as _abspath
from _imre_tempfile import finish_named as _finish_named, keep_named as _keep_named, open_named as _open_named
from _imre_tempfile import mkdir as _mkdir, open_file as _open_file, random_name as _random_name

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
    return _fsencode(_gettempdir())


def gettempprefix():
    return _os.fsdecode(template)


def gettempprefixb():
    return _fsencode(template)


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
    return _create_file(dir, pre, suf, output_type, _os.open, flags, 0o600)


def _create_file(dir, pre, suf, output_type, create, *args):
    dir = _abspath(dir)
    names = _get_candidate_names()
    for _ in range(TMP_MAX):
        path = _candidate_path(dir, pre, suf, next(names), output_type)
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
class _TemporaryFileCloser:
    """A separate object allowing proper closing of a temporary file's
    underlying file object, without adding a __del__ method to the
    temporary file."""

    cleanup_called = False
    close_called = False
    _native_guard = None

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
            if self._native_guard is not None:
                # The Python closer now owns unlinking. A later native guard drop
                # must not unlink a replacement at the same path.
                _keep_named(self._native_guard)
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

def NamedTemporaryFile(mode="w+b", buffering=-1, encoding=None, newline=None,
                       suffix=None, prefix=None, dir=None, delete=True, *,
                       errors=None, delete_on_close=True):
    prefix, suffix, dir, output_type = _sanitize_params(prefix, suffix, dir)
    if not isinstance(mode, str):
        raise TypeError("mode must be a string")
    write_only = mode in ("wb", "bw")
    if not write_only and (len(mode) != 3 or set(mode) not in ({"b", "+", "w"}, {"b", "+", "r"})):
        if len(set(mode)) != len(mode) or any(c not in "xrwabt+" for c in mode):
            raise ValueError(f"invalid mode: {mode!r}")
        if "b" in mode and "t" in mode:
            raise ValueError("can't have text and binary mode at once")
        kinds = sum(c in "xrwa" for c in mode)
        if not kinds:
            raise ValueError("Must have exactly one of create/read/write/append mode and at most one plus")
        if kinds != 1:
            raise ValueError("must have exactly one of create/read/write/append mode")
        raise NotImplementedError("NamedTemporaryFile supports binary write and update modes")
    if buffering != -1:
        raise NotImplementedError("NamedTemporaryFile supports default buffering")
    if encoding is not None:
        raise ValueError("binary mode doesn't take an encoding argument")
    if errors is not None:
        raise ValueError("binary mode doesn't take an errors argument")
    if newline is not None:
        raise ValueError("binary mode doesn't take a newline argument")
    if write_only:
        file, name = _create_file(dir, prefix, suffix, output_type, _open_named, delete_on_close, True)
        try:
            return _TemporaryFileWrapper(file, name, delete, delete_on_close)
        except BaseException:
            try:
                file.close()
            finally:
                _os.unlink(name)
            raise
    (file, closer), name = _create_file(dir, prefix, suffix, output_type, _open_named, delete_on_close)
    try:
        keep = not delete
        wrapper = _TemporaryFileWrapper(file, name, delete, delete_on_close)
        wrapper._closer._native_guard = closer
        if keep:
            _keep_named(closer)
        return wrapper
    except BaseException:
        _finish_named(closer, name, True)
        raise


def TemporaryFile(mode="w+b", buffering=-1, encoding=None, newline=None,
                  suffix=None, prefix=None, dir=None, *, errors=None):
    if buffering != -1:
        raise NotImplementedError("TemporaryFile supports default buffering")
    prefix, suffix, dir, _ = _sanitize_params(prefix, suffix, dir)
    return _open_file(mode, dir=_os.fspath(dir), prefix=prefix, suffix=suffix,
                      encoding=encoding, errors=errors, newline=newline)
