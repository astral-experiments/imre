"""Read resources exposed by a package's loader."""

from importlib import import_module
import sys
import warnings

_missing = object()


def _resolve_anchor(anchor):
    if isinstance(anchor, str):
        return import_module(anchor)
    if anchor is not _missing and anchor is not None:
        return anchor

    frame = sys._getframe()
    filename = frame.f_code.co_filename
    while frame is not None:
        code = frame.f_code
        # CPython excludes this file and singledispatch's wrapper by name.
        if code.co_filename != filename and code.co_name != 'wrapper':
            return _resolve_anchor(frame.f_globals['__name__'])
        frame = frame.f_back
    raise StopIteration


def files(anchor=_missing, *, package=_missing):
    """Return the Traversable root for an anchor, defaulting to the calling module."""
    if package is not _missing:
        if anchor is not _missing:
            raise TypeError("files() received both anchor and package")
        warnings.warn("First parameter to files is renamed to 'anchor'", DeprecationWarning, stacklevel=2)
        anchor = package
    anchor = _resolve_anchor(anchor)
    from ._adapters import wrap_spec

    spec = wrap_spec(anchor)
    reader = spec.loader.get_resource_reader(spec.name)
    return reader.files()


def _get_resource(anchor, path_names):
    if anchor is None:
        raise TypeError("anchor must be module or string, got None")
    return files(anchor).joinpath(*path_names)


def open_binary(anchor, *path_names):
    """Open for binary reading the *resource* within *package*."""
    return _get_resource(anchor, path_names).open('rb')


def open_text(anchor, *path_names, encoding=_missing, errors='strict'):
    """Open for text reading the *resource* within *package*."""
    encoding = _get_encoding_arg(path_names, encoding)
    resource = _get_resource(anchor, path_names)
    return resource.open('r', encoding=encoding, errors=errors)


def read_binary(anchor, *path_names):
    """Read and return contents of *resource* within *package* as bytes."""
    return _get_resource(anchor, path_names).read_bytes()


def read_text(anchor, *path_names, encoding=_missing, errors='strict'):
    """Read a text resource, defaulting to UTF-8 for a single path component."""
    encoding = _get_encoding_arg(path_names, encoding)
    return _get_resource(anchor, path_names).read_text(encoding=encoding, errors=errors)


def _get_encoding_arg(path_names, encoding):
    if encoding is _missing:
        if len(path_names) > 1:
            raise TypeError("'encoding' argument required with multiple path names")
        return 'utf-8'
    return encoding


def path(anchor, *path_names):
    """Return the path to the *resource* as an actual file system path."""
    return as_file(_get_resource(anchor, path_names))


def is_resource(anchor, *path_names):
    """Return ``True`` if there is a resource named *name* in the package,

    Otherwise returns ``False``.
    """
    return _get_resource(anchor, path_names).is_file()


def contents(anchor, *path_names):
    """Return an iterable over the named resources within the package.

    The iterable returns :class:`str` resources (e.g. files).
    The iterable does not recurse into subdirectories.
    """
    warnings.warn(
        "importlib.resources.contents is deprecated. "
        "Use files(anchor).iterdir() instead.",
        DeprecationWarning,
        stacklevel=1,
    )
    return (resource.name for resource in _get_resource(anchor, path_names).iterdir())


def as_file(path):
    """Provide a pathlib.Path for the duration of a context manager."""
    from importlib.resources._as_file import as_file as materialize
    return materialize(path)
