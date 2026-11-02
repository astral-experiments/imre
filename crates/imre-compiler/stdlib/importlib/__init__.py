"""Import modules registered with the native loader."""


from _imre_importlib import import_module as _import_module
from _frozen_importlib import ModuleSpec as _ModuleSpec
import _frozen_importlib_external as _bootstrap_external

_bootstrap_external.__name__ = "importlib._bootstrap_external"
_bootstrap_external.__package__ = "importlib"


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
