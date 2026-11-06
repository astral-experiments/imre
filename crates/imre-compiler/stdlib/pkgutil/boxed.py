"""Discover filesystem modules and read package resources in runtime imports."""

import importlib
import os as _os
import sys
from importlib.util import resolve_name as _spec_name
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


# From CPython 3.14.5 Lib/pkgutil.py. See licenses/CPython.txt.
def get_importer(path_item):
    """Retrieve a finder for the given path item

    The returned finder is cached in sys.path_importer_cache
    if it was newly created by a path hook.

    The cache (or part of it) can be cleared manually if a
    rescan of sys.path_hooks is necessary.
    """
    path_item = _os.fsdecode(path_item)
    try:
        importer = sys.path_importer_cache[path_item]
    except KeyError:
        for path_hook in sys.path_hooks:
            try:
                importer = path_hook(path_item)
                sys.path_importer_cache.setdefault(path_item, importer)
                break
            except ImportError:
                pass
        else:
            importer = None
    return importer


def read_code(stream):
    # This helper is needed in order for the PEP 302 emulation to
    # correctly handle compiled files
    import marshal

    magic = stream.read(4)
    if magic != importlib.util.MAGIC_NUMBER:
        return None

    stream.read(12) # Skip rest of the header
    return marshal.load(stream)


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


# From CPython 3.14.5 Lib/pkgutil.py. See licenses/CPython.txt.
_NAME_PATTERN = None

def resolve_name(name):
    """
    Resolve a name to an object.

    It is expected that `name` will be a string in one of the following
    formats, where W is shorthand for a valid Python identifier and dot stands
    for a literal period in these pseudo-regexes:

    W(.W)*
    W(.W)*:(W(.W)*)?

    The first form is intended for backward compatibility only. It assumes that
    some part of the dotted name is a package, and the rest is an object
    somewhere within that package, possibly nested inside other objects.
    Because the place where the package stops and the object hierarchy starts
    can't be inferred by inspection, repeated attempts to import must be done
    with this form.

    In the second form, the caller makes the division point clear through the
    provision of a single colon: the dotted name to the left of the colon is a
    package to be imported, and the dotted name to the right is the object
    hierarchy within that package. Only one import is needed in this form. If
    it ends with the colon, then a module object is returned.

    The function will return an object (which might be a module), or raise one
    of the following exceptions:

    ValueError - if `name` isn't in a recognised format
    ImportError - if an import failed when it shouldn't have
    AttributeError - if a failure occurred when traversing the object hierarchy
                     within the imported package to get to the desired object.
    """
    global _NAME_PATTERN
    if _NAME_PATTERN is None:
        # Lazy import to speedup Python startup time
        import re
        dotted_words = r'(?!\d)(\w+)(\.(?!\d)(\w+))*'
        _NAME_PATTERN = re.compile(f'^(?P<pkg>{dotted_words})'
                                   f'(?P<cln>:(?P<obj>{dotted_words})?)?$',
                                   re.UNICODE)

    m = _NAME_PATTERN.match(name)
    if not m:
        raise ValueError(f'invalid format: {name!r}')
    gd = m.groupdict()
    if gd.get('cln'):
        # there is a colon - a one-step import is all that's needed
        mod = importlib.import_module(gd['pkg'])
        parts = gd.get('obj')
        parts = parts.split('.') if parts else []
    else:
        # no colon - have to iterate to find the package boundary
        parts = name.split('.')
        modname = parts.pop(0)
        # first part *must* be a module/package.
        mod = importlib.import_module(modname)
        while parts:
            p = parts[0]
            s = f'{modname}.{p}'
            try:
                mod = importlib.import_module(s)
                parts.pop(0)
                modname = s
            except ImportError:
                break
    # if we reach this point, mod is the module, already imported, and
    # parts is the list of parts in the object hierarchy to be traversed, or
    # an empty list if just the module is wanted.
    result = mod
    for p in parts:
        result = getattr(result, p)
    return result
