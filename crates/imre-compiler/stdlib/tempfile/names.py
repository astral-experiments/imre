"""POSIX named temporary files and directories for runtime-loaded modules."""

import os as _os
from errno import EEXIST as _EEXIST

from _imre_tempfile import abspath as _abspath, default_directory as _default_directory
from _imre_tempfile import finish_named as _finish_named, keep_named as _keep_named, open_named as _open_named
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


def _get_candidate_names():
    return _name_sequence


def _fsencode(path):
    path = _os.fspath(path)
    if isinstance(path, str):
        return path.encode("utf-8", "surrogateescape")
    return path


def _gettempdir():
    global tempdir
    if tempdir is None:
        tempdir = _default_directory()
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
    raise FileExistsError(_EEXIST, "No usable temporary file name found")


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
    raise FileExistsError(_EEXIST, "No usable temporary directory name found")


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
