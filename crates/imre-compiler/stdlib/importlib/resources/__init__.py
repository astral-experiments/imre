"""Read resources exposed by a package's loader."""

from importlib import import_module
import warnings

_missing = object()


def files(anchor=_missing, *, package=_missing):
    """Return the Traversable root provided by an explicit anchor's reader."""
    if package is not _missing:
        if anchor is not _missing:
            raise TypeError("files() received both anchor and package")
        warnings.warn("First parameter to files is renamed to 'anchor'", DeprecationWarning, stacklevel=2)
        anchor = package
    if anchor is _missing or anchor is None:
        raise NotImplementedError("inferring the resource anchor from the calling frame is unsupported")
    if isinstance(anchor, str):
        anchor = import_module(anchor)
    from ._adapters import wrap_spec

    spec = wrap_spec(anchor)
    reader = spec.loader.get_resource_reader(spec.name)
    return reader.files()


def _get_resource(anchor, path_names):
    if anchor is None:
        raise TypeError("anchor must be module or string, got None")
    return files(anchor).joinpath(*path_names)


def read_binary(anchor, *path_names):
    """Read and return contents of *resource* within *package* as bytes."""
    return _get_resource(anchor, path_names).read_bytes()


def read_text(anchor, *path_names, encoding=_missing, errors='strict'):
    """Read a text resource, defaulting to UTF-8 for a single path component."""
    if encoding is _missing:
        if len(path_names) > 1:
            raise TypeError("'encoding' argument required with multiple path names")
        encoding = 'utf-8'
    return _get_resource(anchor, path_names).read_text(encoding=encoding, errors=errors)


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
