"""Discover filesystem modules and read package resources in runtime imports."""

import sys
from os import fsdecode
from os.path import abspath as _absolute, dirname, join as _join
from typing import NamedTuple
from _imre_pkgutil import (
    filenames as _filenames,
    find_module as _find_module,
    isdir as _isdir,
    missing_error as _missing_error,
    read_bytes as _read_bytes,
)


class ModuleInfo(NamedTuple):
    module_finder: object
    name: str
    ispkg: bool


def get_data(package, resource):
    """Load a filesystem package and read a resource beside its module file."""
    if package.startswith('.'):
        raise ImportError(f'no package specified for {package!r} (required for relative module names)')
    parent, _, _ = package.rpartition('.')
    if parent:
        module = _find_module(parent)
        if module is None:
            raise _missing_error(parent, f'No module named {parent!r}')
        try:
            module.__path__
        except AttributeError as error:
            raise _missing_error(
                package,
                f'__path__ attribute not found on {parent!r} while trying to find {package!r}',
            ) from error
    filename = getattr(_find_module(package), '__file__', None)
    if filename is None:
        return None
    return _read_bytes(_join(dirname(filename), *resource.split('/')))


def iter_modules(path=None, prefix=''):
    """Yield unique modules and regular packages in filesystem import order."""
    if isinstance(path, str):
        raise ValueError('path must be None or list of paths to look for modules in')
    seen = set()
    for directory in sys.path if path is None else path:
        finder = FileFinder(fsdecode(directory))
        for filename in _filenames(finder.path):
            info = _module_info(finder, filename, prefix, seen)
            if info is not None:
                yield info
