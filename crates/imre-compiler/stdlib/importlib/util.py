"""Finder operations for the standalone native source registry."""

from _frozen_importlib_external import cache_from_source, source_from_cache
from _imre_importlib import find_spec as _find_spec
from importlib import _resolve_relative
from ._abc import Loader


def _spec_name(name, package):
    if name.startswith("."):
        if not package:
            raise ImportError(f"no package specified for {name!r} (required for relative module names)")
        return _resolve_relative(name, package)
    return name


def find_spec(name, package=None):
    return _find_spec(_spec_name(name, package), name)
