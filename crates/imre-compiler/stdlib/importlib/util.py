"""Finder operations for the standalone native source registry."""

from ._bootstrap import _find_spec, module_from_spec, spec_from_loader
from ._bootstrap_external import cache_from_source, source_from_cache, spec_from_file_location
from _imre_importlib import find_spec as _find_cached_spec
from importlib import _resolve_relative
from ._abc import Loader


def _spec_name(name, package):
    if name.startswith("."):
        if not package:
            raise ImportError(f"no package specified for {name!r} (required for relative module names)")
        return _resolve_relative(name, package)
    return name


def find_spec(name, package=None):
    return _find_cached_spec(_spec_name(name, package), name, _find_spec)
