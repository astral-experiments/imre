"""POSIX named temporary files and directories for runtime-loaded modules."""

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


class _TemporaryFileWrapper:
    def __init__(self, file, name, closer):
        self.file = file
        self.name = name
        self._closer = closer
        self._original_name = name

    def __getattr__(self, name):
        value = getattr(self.file, name)
        if callable(value):
            def call(*args, **kwargs):
                return value(*args, **kwargs)
            call._closer = self._closer
            setattr(self, name, call)
            return call
        return value

    def close(self):
        _finish_named(self._closer, self._original_name, False)

    def __enter__(self):
        self.file.__enter__()
        return self

    def __exit__(self, exc_type, exc, tb):
        _finish_named(self._closer, self._original_name, True)


def NamedTemporaryFile(mode="w+b", buffering=-1, encoding=None, newline=None,
                       suffix=None, prefix=None, dir=None, delete=True, *,
                       errors=None, delete_on_close=True):
    prefix, suffix, dir, output_type = _sanitize_params(prefix, suffix, dir)
    if not isinstance(mode, str):
        raise TypeError("mode must be a string")
    if len(mode) != 3 or set(mode) not in ({"b", "+", "w"}, {"b", "+", "r"}):
        if len(set(mode)) != len(mode) or any(c not in "xrwabt+" for c in mode):
            raise ValueError(f"invalid mode: {mode!r}")
        if "b" in mode and "t" in mode:
            raise ValueError("can't have text and binary mode at once")
        kinds = sum(c in "xrwa" for c in mode)
        if not kinds:
            raise ValueError("Must have exactly one of create/read/write/append mode and at most one plus")
        if kinds != 1:
            raise ValueError("must have exactly one of create/read/write/append mode")
        raise NotImplementedError("NamedTemporaryFile supports binary update mode")
    if buffering != -1:
        raise NotImplementedError("NamedTemporaryFile supports default buffering")
    if encoding is not None:
        raise ValueError("binary mode doesn't take an encoding argument")
    if errors is not None:
        raise ValueError("binary mode doesn't take an errors argument")
    if newline is not None:
        raise ValueError("binary mode doesn't take a newline argument")
    (file, closer), name = _create_file(dir, prefix, suffix, output_type, _open_named, delete_on_close)
    try:
        wrapper = _TemporaryFileWrapper(file, name, closer)
        if not delete:
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
