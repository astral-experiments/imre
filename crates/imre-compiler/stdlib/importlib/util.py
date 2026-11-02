"""Finder operations for the standalone native source registry."""

from ._bootstrap import _find_spec, _resolve_name, module_from_spec, spec_from_loader
from ._bootstrap_external import MAGIC_NUMBER, cache_from_source, source_from_cache, spec_from_file_location
from _imre_importlib import find_spec as _find_cached_spec
from ._abc import Loader


def resolve_name(name, package):
    """Resolve a relative module name to an absolute one."""
    if not name.startswith("."):
        return name
    if not package:
        raise ImportError(f"no package specified for {name!r} (required for relative module names)")
    level = 0
    for character in name:
        if character != ".":
            break
        level += 1
    return _resolve_name(name[level:], package, level)


def find_spec(name, package=None):
    fullname = resolve_name(name, package) if name.startswith(".") else name
    return _find_cached_spec(fullname, name, _find_spec)
