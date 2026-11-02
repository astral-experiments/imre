"""Legacy direct loading with a SourceFileLoader, including replacement modules."""

import sys
import _frozen_importlib_external as _bootstrap_external
from _frozen_importlib import ModuleSpec
from _imre_importlib import warn_legacy_load
from os import altsep, sep


def _new_module(name):
    return type(sys)(name)


def _loader_spec(name, loader):
    origin = getattr(loader, "_ORIGIN", None)
    file_location = not origin and hasattr(loader, "get_filename")
    if file_location:
        origin = "<unknown>"
        try:
            origin = loader.get_filename(name)
        except ImportError:
            pass

    spec = ModuleSpec(name, loader, origin=origin)
    spec.has_location = bool(file_location)
    if hasattr(loader, "is_package"):
        try:
            is_package = loader.is_package(name)
        except ImportError:
            pass
        else:
            if is_package:
                spec.submodule_search_locations = []
                if file_location and origin:
                    index = origin.rfind(sep)
                    if altsep is not None:
                        index = max(index, origin.rfind(altsep))
                    spec.submodule_search_locations.append(origin[:index] if index >= 0 else "")
    return spec


def _move_to_end(spec):
    module = sys.modules.pop(spec.name)
    sys.modules[spec.name] = module
    return module


def load(loader, fullname):
    warn_legacy_load()
    spec = _loader_spec(fullname, loader)
    if fullname in sys.modules:
        module = sys.modules[fullname]
        if sys.modules.get(spec.name) is not module:
            raise ImportError(f"module {spec.name!r} not in sys.modules", name=spec.name)
        try:
            _init_module_attrs(spec, module, override=True)
            spec.loader.exec_module(module)
        finally:
            _move_to_end(spec)
        return sys.modules[fullname]

    module = module_from_spec(spec)
    spec._initializing = True
    try:
        sys.modules[spec.name] = module
        try:
            spec.loader.exec_module(module)
        except:
            try:
                del sys.modules[spec.name]
            except KeyError:
                pass
            raise
        return _move_to_end(spec)
    finally:
        spec._initializing = False
