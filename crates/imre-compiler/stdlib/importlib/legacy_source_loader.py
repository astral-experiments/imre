"""Legacy direct loading with a SourceFileLoader, including replacement modules."""

import sys
from _imre_importlib import import_module as _native_import, warn_legacy_load as _warn_legacy_load
from os import altsep, sep


_NEEDS_LOADING = object()


def _warning_module():
    import warnings
    return warnings


_is_frozen = __name__ == "_frozen_importlib"
if not _is_frozen:
    _bootstrap_external = None


def _setup(sys_module, _imp_module):
    # Importer state and module locking are owned by the native resolver.
    global sys, _imp
    sys = sys_module
    _imp = _imp_module


def _find_and_load(name, import_):
    return _native_import(name)


def _new_module(name):
    return type(sys)(name)


def _loader_spec(name, loader):
    origin = getattr(loader, "_ORIGIN", None)
    file_location = not origin and hasattr(loader, "get_filename")
    if file_location:
        origin = "<unknown>"
        # spec_from_file_location probes again after spec_from_loader's dispatch.
        if hasattr(loader, "get_filename"):
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


def _load_module_shim(loader, fullname):
    _warn_legacy_load(None if _is_frozen else globals())
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
            if spec.loader is None:
                if spec.submodule_search_locations is None:
                    raise ImportError("missing loader", name=spec.name)
            else:
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
