"""Import modules registered with the native loader."""


import _imp
import sys
from _imre_importlib import namespace_locations as _namespace_locations

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
    for finder in sys.meta_path:
        if hasattr(finder, "invalidate_caches"):
            finder.invalidate_caches()


def _file_spec(name, origin, locations, loader=None):
    if loader is None:
        loader = _bootstrap_external.SourceFileLoader(name, origin)
    spec = _ModuleSpec(name, loader, origin=origin)
    spec.submodule_search_locations = locations
    spec.has_location = True
    return spec


def _find_namespace_portions(name, path):
    locations = _namespace_locations(name, path)
    if locations is not None:
        spec = _ModuleSpec(name, None)
        spec.submodule_search_locations = locations
        return spec


def _namespace_spec(name, locations, module, source_globals):
    # The installed finder supplies the spec and path. The importing core installs the loader.
    finder = _bootstrap_external._finder_external
    spec = finder._bootstrap.ModuleSpec(name, None)
    spec.submodule_search_locations = finder._NamespacePath(name, locations, _find_namespace_portions)
    if module is not None:
        init_attrs = (finder._bootstrap._init_module_attrs if source_globals is None
                      else source_globals["_init_module_attrs"])
        init_attrs(spec, module, override=True)
        spec._initializing = False
    return spec


def import_module(name, package=None):
    """Import a module by absolute name or relative to package."""
    level = 0
    if name.startswith("."):
        if not package:
            raise TypeError(f"the 'package' argument is required to perform a relative import for {name!r}")
        for character in name:
            if character != ".":
                break
            level += 1
    return _bootstrap._gcd_import(name[level:], package, level)
