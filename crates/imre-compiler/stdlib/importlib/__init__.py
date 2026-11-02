"""Import modules registered with the native loader."""


import _imp
import sys
from _imre_importlib import import_module as _import_module

try:
    import _frozen_importlib as _bootstrap
except ImportError:
    from . import _bootstrap
    _bootstrap._setup(sys, _imp)
else:
    _bootstrap.__name__ = "importlib._bootstrap"
    _bootstrap.__package__ = "importlib"
    sys.modules["importlib._bootstrap"] = _bootstrap

try:
    import _frozen_importlib_external as _bootstrap_external
except ImportError:
    from . import _bootstrap_external
    _bootstrap_external._set_bootstrap_module(_bootstrap)
    _bootstrap._bootstrap_external = _bootstrap_external
else:
    _bootstrap_external.__name__ = "importlib._bootstrap_external"
    _bootstrap_external.__package__ = "importlib"
    sys.modules["importlib._bootstrap_external"] = _bootstrap_external

__import__ = _bootstrap.__import__
_ModuleSpec = _bootstrap.ModuleSpec


def invalidate_caches():
    """Call each meta path finder's cache invalidation hook when present."""
    default = object()
    finders = getattr(sys, "meta_path", default)
    if finders is default:
        # The closed native resolver has no public default meta path.
        return _bootstrap_external._invalidate_default_path_caches()
    for finder in finders:
        if hasattr(finder, "invalidate_caches"):
            finder.invalidate_caches()


def _file_spec(name, origin, locations, loader=None):
    if loader is None:
        loader = _bootstrap_external.SourceFileLoader(name, origin)
    spec = _ModuleSpec(name, loader, origin=origin)
    spec.submodule_search_locations = locations
    spec.has_location = True
    return spec


def _resolve_relative(name, package):
    rest = name.lstrip(".")
    level = len(name) - len(rest)
    bits = package.rsplit(".", level - 1)
    if len(bits) < level:
        raise ImportError("attempted relative import beyond top-level package")
    return bits[0] + "." + rest if rest else bits[0]


def import_module(name, package=None):
    """Import a module by absolute name or relative to package."""
    if name.startswith("."):
        if not package:
            raise TypeError(f"the 'package' argument is required to perform a relative import for {name!r}")
        if not isinstance(package, str):
            raise TypeError("__package__ not set to a string")
        name = _resolve_relative(name, package)
    elif not name:
        raise ValueError("Empty module name")
    return _import_module(name)
