"""Legacy direct loading with a SourceFileLoader, including replacement modules."""

import sys
from _thread import RLock
from _imre_importlib import (
    _import_lock,
    is_builtin_module as _is_builtin_module,
    frozen_info as _frozen_info,
    frozen_code as _frozen_code,
    create_builtin_module as _create_builtin_module,
    exec_builtin_module as _exec_builtin_module,
    create_native_module as _create_native_module,
    exec_native_module as _exec_native_module,
    native_spec_info as _native_spec_info,
    native_code as _native_code,
    native_source as _native_source,
    warn_import as _warn_import,
)
from _imre_importlib import import_module as _native_import, warn_legacy_load as _warn_legacy_load


# The bootstrap runs before high-level filesystem modules are available.
sep, altsep = ("\\", "/") if sys.platform == "win32" else ("/", None)

_NEEDS_LOADING = object()
_module_locks = {}


class _ModuleLockManager:
    """Hold a reentrant lock for this name within the bootstrap's module registry."""

    def __init__(self, name):
        self._name = name

    def __enter__(self):
        # Keep the entry alive for waiters, but release the registry lock before
        # acquiring the module lock or running any loader code.
        with _import_lock:
            lock, users = _module_locks.get(self._name, (None, 0))
            self._lock = lock if lock is not None else RLock()
            _module_locks[self._name] = (self._lock, users + 1)
        try:
            self._lock.acquire()
        except BaseException:
            self._drop_user()
            raise

    def _drop_user(self):
        with _import_lock:
            lock, users = _module_locks[self._name]
            if users == 1:
                del _module_locks[self._name]
            else:
                _module_locks[self._name] = (lock, users - 1)

    def __exit__(self, *exc):
        self._lock.release()
        self._drop_user()


def _lock_unlock_module(name):
    with _ModuleLockManager(name):
        pass


def _warning_module():
    import warnings
    return warnings


_is_frozen = __name__ == "_frozen_importlib"
if not _is_frozen or "_bootstrap_external" not in globals():
    _bootstrap_external = None


def _setup(sys_module, _imp_module):
    # The native resolver owns the active finder registry.
    global sys, _imp
    sys = sys_module
    _imp = _imp_module


def _find_and_load(name, import_):
    return _native_import(name, None if _is_frozen else globals())


def _import_warning(message, line):
    _warn_import(None if _is_frozen else globals(), message, line)


def _require_builtin(fullname):
    if not isinstance(fullname, str) or not _is_builtin_module(fullname):
        raise ImportError(f'{fullname!r} is not a built-in module', name=fullname)


class BuiltinImporter:
    """Find and initialize modules built into this interpreter."""

    _ORIGIN = "built-in"

    @classmethod
    def find_spec(cls, fullname, path=None, target=None):
        if _is_builtin_module(fullname):
            return ModuleSpec(fullname, cls, origin=cls._ORIGIN)
        return None

    @staticmethod
    def create_module(spec):
        name = spec.name
        _require_builtin(name)
        return _create_builtin_module(name)

    @staticmethod
    def exec_module(module):
        _exec_builtin_module(module)

    @classmethod
    def get_code(cls, fullname):
        _require_builtin(fullname)
        return None

    @classmethod
    def get_source(cls, fullname):
        _require_builtin(fullname)
        return None

    @classmethod
    def is_package(cls, fullname):
        _require_builtin(fullname)
        return False

    @classmethod
    def load_module(cls, fullname):
        return _load_module_shim(cls, fullname)


def _frozen_package(name):
    return _frozen_info(name)[1]


def _init_startup_specs(sys_functions):
    # Recreating single-phase modules restores their initial exports. Interpreter
    # configuration and streams belong to the running interpreter's sys object.
    global _core_exports
    methods = {name: getattr(sys, name) for name in sys_functions}
    _core_exports = {'sys': methods, 'builtins': vars(sys.modules['builtins']).copy()}
    for name, values in _core_exports.items():
        values.update(__name__=name, __doc__=sys.modules[name].__doc__,
                      __package__=None, __loader__=None, __spec__=None)
    for name, module in tuple(sys.modules.items()):
        spec = BuiltinImporter.find_spec(name)
        if spec is None:
            spec = FrozenImporter.find_spec(name)
        if spec is not None:
            _init_module_attrs(spec, module)


def _create_core_module(name):
    module = sys.modules.get(name)
    if not isinstance(module, type(sys)):
        module = _new_module(name)
        sys.modules[name] = module
    vars(module).update(_core_exports[name])
    return module


class _NativeImporter:
    """Find and initialize the modules provided by this native interpreter."""

    @classmethod
    def find_spec(cls, fullname, path=None, target=None):
        info = _native_spec_info(fullname, path, cls is _BundledImporter)
        if info is None:
            if cls is _BundledImporter and path is None and _bootstrap_external is not None:
                return _bootstrap_external._installed_source_spec(fullname, '<imre stdlib>', target)
            return None
        origin, locations = info
        spec = ModuleSpec(fullname, cls, origin=origin)
        spec.submodule_search_locations = locations
        return spec

    @classmethod
    def get_source(cls, fullname):
        return _native_source(fullname, cls is _BundledImporter)

    @classmethod
    def get_code(cls, fullname):
        return _native_code(fullname, cls is _BundledImporter)

    @staticmethod
    def create_module(spec):
        return _create_native_module(spec.name)

    @staticmethod
    def exec_module(module):
        _exec_native_module(module)


class _BundledImporter(_NativeImporter):
    """Find bundled Python sources after entries on the application's path."""


_native_find_spec = _NativeImporter.__dict__['find_spec']


def _prefer_bundled_spec(finder, spec, name, path, target):
    # PathFinder has exhausted regular modules on the application path. The
    # adjacent bundled finder supplies Python sources from the virtual stdlib.
    if (spec is not None and _bootstrap_external is not None
            and finder is _bootstrap_external._registered_path_finder
            and finder.__dict__.get('find_spec') is _bootstrap_external._registered_find_spec
            and spec.loader is None and spec.submodule_search_locations is not None
            and 'find_spec' not in _BundledImporter.__dict__
            and _NativeImporter.__dict__.get('find_spec') is _native_find_spec):
        bundled = _BundledImporter.find_spec(name, path, target)
        if bundled is not None:
            return bundled
    return spec


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
