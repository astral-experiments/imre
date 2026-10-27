"""Import modules registered with the native loader."""


from _frozen_importlib import ModuleSpec as _ModuleSpec
from _frozen_importlib_external import SourceFileLoader as _SourceFileLoader


def _file_spec(name, origin, locations, loader=None):
    if loader is None:
        loader = _SourceFileLoader(name, origin)
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
    # A nonempty fromlist returns the requested module, including a dotted child.
    return __import__(name, fromlist=("__name__",))
