"""Discover filesystem modules and read package resources in runtime imports."""

import sys
from importlib.util import _spec_name
from os import fsdecode
from os.path import abspath as _absolute, dirname, join as _join
from typing import NamedTuple
from _imre_pkgutil import (
    filenames as _filenames,
    find_module as _find_module,
    find_spec as _find_spec,
    isdir as _isdir,
)


class ModuleInfo(NamedTuple):
    module_finder: object
    name: str
    ispkg: bool


def get_data(package, resource):
    """Load a filesystem package and read a resource beside its module file."""
    spec = _find_spec(_spec_name(package, None))
    if spec is None or not hasattr(spec.loader, 'get_data'):
        return None
    filename = getattr(_find_module(package), '__file__', None)
    if filename is None:
        return None
    return spec.loader.get_data(_join(dirname(filename), *resource.split('/')))


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
