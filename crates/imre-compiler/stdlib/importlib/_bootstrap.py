"""Legacy direct loading with a SourceFileLoader, including replacement modules."""

import sys
from _thread import RLock
from _imre_importlib import (
    _import_lock,
    is_builtin_module as _is_builtin_module,
    builtin_names_contains as _builtin_names_contains,
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
    if not _builtin_names_contains(sys, fullname):
        raise ImportError(f'{fullname!r} is not a built-in module', name=fullname)


class BuiltinImporter:
    """Find and initialize modules built into this interpreter."""

    _ORIGIN = "built-in"

    @classmethod
    def find_spec(cls, fullname, path=None, target=None):
        if _is_builtin_module(fullname):
            return spec_from_loader(fullname, cls, origin=cls._ORIGIN)
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

def _module_repr(module):
    """The implementation of ModuleType.__repr__()."""
    loader = getattr(module, '__loader__', None)
    if spec := getattr(module, "__spec__", None):
        return _module_repr_from_spec(spec)
    # Fall through to a catch-all which always succeeds.
    try:
        name = module.__name__
    except AttributeError:
        name = '?'
    try:
        filename = module.__file__
    except AttributeError:
        if loader is None:
            return f'<module {name!r}>'
        else:
            return f'<module {name!r} ({loader!r})>'
    else:
        return f'<module {name!r} from {filename!r}>'


class ModuleSpec:
    """The specification for a module, used for loading.

    A module's spec is the source for information about the module.  For
    data associated with the module, including source, use the spec's
    loader.

    `name` is the absolute name of the module.  `loader` is the loader
    to use when loading the module.  `parent` is the name of the
    package the module is in.  The parent is derived from the name.

    `is_package` determines if the module is considered a package or
    not.  On modules this is reflected by the `__path__` attribute.

    `origin` is the specific location used by the loader from which to
    load the module, if that information is available.  When filename is
    set, origin will match.

    `has_location` indicates that a spec's "origin" reflects a location.
    When this is True, `__file__` attribute of the module is set.

    `cached` is the location of the cached bytecode file, if any.  It
    corresponds to the `__cached__` attribute.

    `submodule_search_locations` is the sequence of path entries to
    search when importing submodules.  If set, is_package should be
    True--and False otherwise.

    Packages are simply modules that (may) have submodules.  If a spec
    has a non-None value in `submodule_search_locations`, the import
    system will consider modules loaded from the spec as packages.

    Only finders (see importlib.abc.MetaPathFinder and
    importlib.abc.PathEntryFinder) should modify ModuleSpec instances.

    """

    def __init__(self, name, loader, *, origin=None, loader_state=None,
                 is_package=None):
        self.name = name
        self.loader = loader
        self.origin = origin
        self.loader_state = loader_state
        self.submodule_search_locations = [] if is_package else None
        self._uninitialized_submodules = []

        # file-location attributes
        self._set_fileattr = False
        self._cached = None

    def __repr__(self):
        args = [f'name={self.name!r}', f'loader={self.loader!r}']
        if self.origin is not None:
            args.append(f'origin={self.origin!r}')
        if self.submodule_search_locations is not None:
            args.append(f'submodule_search_locations={self.submodule_search_locations}')
        return f'{self.__class__.__name__}({", ".join(args)})'

    def __eq__(self, other):
        smsl = self.submodule_search_locations
        try:
            return (self.name == other.name and
                    self.loader == other.loader and
                    self.origin == other.origin and
                    smsl == other.submodule_search_locations and
                    self.cached == other.cached and
                    self.has_location == other.has_location)
        except AttributeError:
            return NotImplemented

    @property
    def cached(self):
        if self._cached is None:
            if self.origin is not None and self._set_fileattr:
                if _bootstrap_external is None:
                    raise NotImplementedError
                self._cached = _bootstrap_external._get_cached(self.origin)
        return self._cached

    @cached.setter
    def cached(self, cached):
        self._cached = cached

    @property
    def parent(self):
        """The name of the module's parent."""
        if self.submodule_search_locations is None:
            return self.name.rpartition('.')[0]
        else:
            return self.name

    @property
    def has_location(self):
        return self._set_fileattr

    @has_location.setter
    def has_location(self, value):
        self._set_fileattr = bool(value)


def spec_from_loader(name, loader, *, origin=None, is_package=None):
    """Return a module spec based on various loader methods."""
    if origin is None:
        origin = getattr(loader, '_ORIGIN', None)

    if not origin and hasattr(loader, 'get_filename'):
        if _bootstrap_external is None:
            raise NotImplementedError
        spec_from_file_location = _bootstrap_external.spec_from_file_location

        if is_package is None:
            return spec_from_file_location(name, loader=loader)
        search = [] if is_package else None
        return spec_from_file_location(name, loader=loader,
                                       submodule_search_locations=search)

    if is_package is None:
        if hasattr(loader, 'is_package'):
            try:
                is_package = loader.is_package(name)
            except ImportError:
                is_package = None  # aka, undefined
        else:
            # the default
            is_package = False

    return ModuleSpec(name, loader, origin=origin, is_package=is_package)



def _init_module_attrs(spec, module, *, override=False):
    # The passed-in module may be not support attribute assignment,
    # in which case we simply don't set the attributes.
    # __name__
    if (override or getattr(module, '__name__', None) is None):
        try:
            module.__name__ = spec.name
        except AttributeError:
            pass
    # __loader__
    if override or getattr(module, '__loader__', None) is None:
        loader = spec.loader
        if loader is None:
            # A backward compatibility hack.
            if spec.submodule_search_locations is not None:
                if _bootstrap_external is None:
                    raise NotImplementedError
                NamespaceLoader = _bootstrap_external.NamespaceLoader

                loader = NamespaceLoader.__new__(NamespaceLoader)
                loader._path = spec.submodule_search_locations
                spec.loader = loader
                # While the docs say that module.__file__ is not set for
                # built-in modules, and the code below will avoid setting it if
                # spec.has_location is false, this is incorrect for namespace
                # packages.  Namespace packages have no location, but their
                # __spec__.origin is None, and thus their module.__file__
                # should also be None for consistency.  While a bit of a hack,
                # this is the best place to ensure this consistency.
                #
                # See # https://docs.python.org/3/library/importlib.html#importlib.abc.Loader.load_module
                # and bpo-32305
                module.__file__ = None
        try:
            module.__loader__ = loader
        except AttributeError:
            pass
    # __package__
    if override or getattr(module, '__package__', None) is None:
        try:
            module.__package__ = spec.parent
        except AttributeError:
            pass
    # __spec__
    try:
        module.__spec__ = spec
    except AttributeError:
        pass
    # __path__
    if override or getattr(module, '__path__', None) is None:
        if spec.submodule_search_locations is not None:
            # XXX We should extend __path__ if it's already a list.
            try:
                module.__path__ = spec.submodule_search_locations
            except AttributeError:
                pass
    # __file__/__cached__
    if spec.has_location:
        if override or getattr(module, '__file__', None) is None:
            try:
                module.__file__ = spec.origin
            except AttributeError:
                pass

        if override or getattr(module, '__cached__', None) is None:
            if spec.cached is not None:
                try:
                    module.__cached__ = spec.cached
                except AttributeError:
                    pass
    return module


def module_from_spec(spec):
    """Create a module based on the provided spec."""
    # Typically loaders will not implement create_module().
    module = None
    if hasattr(spec.loader, 'create_module'):
        # If create_module() returns `None` then it means default
        # module creation should be used.
        module = spec.loader.create_module(spec)
    elif hasattr(spec.loader, 'exec_module'):
        raise ImportError('loaders that define exec_module() '
                          'must also define create_module()')
    if module is None:
        module = _new_module(spec.name)
    _init_module_attrs(spec, module)
    return module


def _module_repr_from_spec(spec):
    """Return the repr to use for the module."""
    name = '?' if spec.name is None else spec.name
    if spec.origin is None:
        loader = spec.loader
        if loader is None:
            return f'<module {name!r}>'
        elif (
            _bootstrap_external is not None
            and isinstance(loader, _bootstrap_external.NamespaceLoader)
        ):
            return f'<module {name!r} (namespace) from {list(loader._path)}>'
        else:
            return f'<module {name!r} ({loader!r})>'
    else:
        if spec.has_location:
            return f'<module {name!r} from {spec.origin!r}>'
        else:
            return f'<module {spec.name!r} ({spec.origin})>'


# Hide bootstrap implementation details when reporting import failures.

def _call_with_frames_removed(f, *args, **kwds):
    """remove_importlib_frames in import.c will always remove sequences
    of importlib frames that end with a call to this function

    Use it instead of a normal call in places where including the importlib
    frames introduces unwanted noise into the traceback (e.g. when executing
    module code)
    """
    return f(*args, **kwds)


def _verbose_message(message, *args, verbosity=1):
    """Print the message to stderr if -v/PYTHONVERBOSE is turned on."""
    if sys.flags.verbose >= verbosity:
        if not message.startswith(('#', 'import ')):
            message = '# ' + message
        print(message.format(*args), file=sys.stderr)



def _wrap(new, old):
    """Simple substitute for functools.update_wrapper."""
    for replace in ['__module__', '__name__', '__qualname__', '__doc__']:
        if hasattr(old, replace):
            setattr(new, replace, getattr(old, replace))
    new.__dict__.update(old.__dict__)



def _requires_frozen(fxn):
    """Decorator to verify the named module is frozen."""
    def _requires_frozen_wrapper(self, fullname):
        if _frozen_info(fullname) is None:
            raise ImportError(f'{fullname!r} is not a frozen module',
                              name=fullname)
        return fxn(self, fullname)
    _wrap(_requires_frozen_wrapper, fxn)
    return _requires_frozen_wrapper


# FrozenImporter reads code and module metadata from the registered provider.

class FrozenImporter:

    """Meta path import for frozen modules.

    All methods are either class or static methods to avoid the need to
    instantiate the class.

    """

    _ORIGIN = "frozen"

    @classmethod
    def _fix_up_module(cls, module):
        spec = module.__spec__
        state = spec.loader_state
        if state is None:
            # The module is missing FrozenImporter-specific values.

            # Fix up the spec attrs.
            origname = vars(module).pop('__origname__', None)
            assert origname, 'see PyImport_ImportFrozenModuleObject()'
            ispkg = hasattr(module, '__path__')
            assert _frozen_package(module.__name__) == ispkg, ispkg
            filename, pkgdir = cls._resolve_filename(origname, spec.name, ispkg)
            spec.loader_state = type(sys.implementation)(
                filename=filename,
                origname=origname,
            )
            __path__ = spec.submodule_search_locations
            if ispkg:
                assert __path__ == [], __path__
                if pkgdir:
                    spec.submodule_search_locations.insert(0, pkgdir)
            else:
                assert __path__ is None, __path__

            # Fix up the module attrs (the bare minimum).
            assert not hasattr(module, '__file__'), module.__file__
            if filename:
                try:
                    module.__file__ = filename
                except AttributeError:
                    pass
            if ispkg:
                if module.__path__ != __path__:
                    assert module.__path__ == [], module.__path__
                    module.__path__.extend(__path__)
        else:
            # These checks ensure that _fix_up_module() is only called
            # in the right places.
            __path__ = spec.submodule_search_locations
            ispkg = __path__ is not None
            # Check the loader state.
            assert sorted(vars(state)) == ['filename', 'origname'], state
            if state.origname:
                # The only frozen modules with "origname" set are stdlib modules.
                (__file__, pkgdir,
                 ) = cls._resolve_filename(state.origname, spec.name, ispkg)
                assert state.filename == __file__, (state.filename, __file__)
                if pkgdir:
                    assert __path__ == [pkgdir], (__path__, pkgdir)
                else:
                    assert __path__ == ([] if ispkg else None), __path__
            else:
                __file__ = None
                assert state.filename is None, state.filename
                assert __path__ == ([] if ispkg else None), __path__
            # Check the file attrs.
            if __file__:
                assert hasattr(module, '__file__')
                assert module.__file__ == __file__, (module.__file__, __file__)
            else:
                assert not hasattr(module, '__file__'), module.__file__
            if ispkg:
                assert hasattr(module, '__path__')
                assert module.__path__ == __path__, (module.__path__, __path__)
            else:
                assert not hasattr(module, '__path__'), module.__path__
        assert not spec.has_location

    @classmethod
    def _resolve_filename(cls, fullname, alias=None, ispkg=False):
        if not fullname or not getattr(sys, '_stdlib_dir', None):
            return None, None
        try:
            sep = cls._SEP
        except AttributeError:
            sep = cls._SEP = '\\' if sys.platform == 'win32' else '/'

        if fullname != alias:
            if fullname.startswith('<'):
                fullname = fullname[1:]
                if not ispkg:
                    fullname = f'{fullname}.__init__'
            else:
                ispkg = False
        relfile = fullname.replace('.', sep)
        if ispkg:
            pkgdir = f'{sys._stdlib_dir}{sep}{relfile}'
            filename = f'{pkgdir}{sep}__init__.py'
        else:
            pkgdir = None
            filename = f'{sys._stdlib_dir}{sep}{relfile}.py'
        return filename, pkgdir

    @classmethod
    def find_spec(cls, fullname, path=None, target=None):
        info = _call_with_frames_removed(_frozen_info, fullname)
        if info is None:
            return None
        # We get the marshaled data in exec_module() (the loader
        # part of the importer), instead of here (the finder part).
        # The loader is the usual place to get the data that will
        # be loaded into the module.  (For example, see _LoaderBasics
        # in _bootstrap_external.py.)  Most importantly, this importer
        # is simpler if we wait to get the data.
        # However, getting as much data in the finder as possible
        # to later load the module is okay, and sometimes important.
        # (That's why ModuleSpec.loader_state exists.)  This is
        # especially true if it avoids throwing away expensive data
        # the loader would otherwise duplicate later and can be done
        # efficiently.  In this case it isn't worth it.
        _, ispkg, origname = info
        spec = spec_from_loader(fullname, cls,
                                origin=cls._ORIGIN,
                                is_package=ispkg)
        filename, pkgdir = cls._resolve_filename(origname, fullname, ispkg)
        spec.loader_state = type(sys.implementation)(
            filename=filename,
            origname=origname,
        )
        if pkgdir:
            spec.submodule_search_locations.insert(0, pkgdir)
        return spec

    @staticmethod
    def create_module(spec):
        """Set __file__, if able."""
        module = _new_module(spec.name)
        try:
            filename = spec.loader_state.filename
        except AttributeError:
            pass
        else:
            if filename:
                module.__file__ = filename
        return module

    @staticmethod
    def exec_module(module):
        spec = module.__spec__
        name = spec.name
        code = _call_with_frames_removed(_frozen_code, name)
        exec(code, module.__dict__)

    @classmethod
    def load_module(cls, fullname):
        """Load a frozen module.

        This method is deprecated.  Use exec_module() instead.

        """
        # Warning about deprecation implemented in _load_module_shim().
        module = _load_module_shim(cls, fullname)
        info = _frozen_info(fullname)
        assert info is not None
        _, ispkg, origname = info
        module.__origname__ = origname
        vars(module).pop('__file__', None)
        if ispkg:
            module.__path__ = []
        cls._fix_up_module(module)
        return module

    @classmethod
    @_requires_frozen
    def get_code(cls, fullname):
        """Return the code object for the frozen module."""
        return _frozen_code(fullname)

    @classmethod
    @_requires_frozen
    def get_source(cls, fullname):
        """Return None as frozen modules do not have source code."""
        return None

    @classmethod
    @_requires_frozen
    def is_package(cls, fullname):
        """Return True if the frozen module is a package."""
        return _frozen_package(fullname)



def _resolve_name(name, package, level):
    """Resolve a relative module name to an absolute one."""
    bits = package.rsplit('.', level - 1)
    if len(bits) < level:
        raise ImportError('attempted relative import beyond top-level package')
    base = bits[0]
    return f'{base}.{name}' if name else base



def _sanity_check(name, package, level):
    """Verify arguments are "sane"."""
    if not isinstance(name, str):
        raise TypeError(f'module name must be str, not {type(name)}')
    if level < 0:
        raise ValueError('level must be >= 0')
    if level > 0:
        if not isinstance(package, str):
            raise TypeError('__package__ not set to a string')
        elif not package:
            raise ImportError('attempted relative import with no known parent '
                              'package')
    if not name and level == 0:
        raise ValueError('Empty module name')



def _object_name(obj):
    try:
        return obj.__qualname__
    except AttributeError:
        return type(obj).__qualname__


def _find_spec(name, path, target=None):
    """Find a module's spec."""
    meta_path = sys.meta_path
    if meta_path is None:
        raise ImportError("sys.meta_path is None, Python is likely "
                          "shutting down")

    # gh-130094: Copy sys.meta_path so that we have a consistent view of the
    # list while iterating over it.
    meta_path = list(meta_path)
    if not meta_path:
        _import_warning('sys.meta_path is empty', 1254)

    # We check sys.modules here for the reload case.  While a passed-in
    # target will usually indicate a reload there is no guarantee, whereas
    # sys.modules provides one.
    is_reload = name in sys.modules
    for index, finder in enumerate(meta_path):
        with _import_lock:
            try:
                find_spec = finder.find_spec
            except AttributeError:
                continue
            else:
                spec = find_spec(name, path, target)
                if index + 1 < len(meta_path) and meta_path[index + 1] is _BundledImporter:
                    spec = _prefer_bundled_spec(finder, spec, name, path, target)
        if spec is not None:
            # The parent import may have already imported this module.
            if not is_reload and name in sys.modules:
                module = sys.modules[name]
                try:
                    __spec__ = module.__spec__
                except AttributeError:
                    # We use the found spec since that is the one that
                    # we would have used if the parent module hadn't
                    # beaten us to the punch.
                    return spec
                else:
                    if __spec__ is None:
                        return spec
                    else:
                        return __spec__
            else:
                return spec
    else:
        return None



def _exec(spec, module):
    """Execute the spec's specified module in an existing module's namespace."""
    name = spec.name
    with _ModuleLockManager(name):
        if sys.modules.get(name) is not module:
            msg = f'module {name!r} not in sys.modules'
            raise ImportError(msg, name=name)
        try:
            if spec.loader is None:
                if spec.submodule_search_locations is None:
                    raise ImportError('missing loader', name=spec.name)
                # Namespace package.
                _init_module_attrs(spec, module, override=True)
            else:
                _init_module_attrs(spec, module, override=True)
                if not hasattr(spec.loader, 'exec_module'):
                    msg = (f"{_object_name(spec.loader)}.exec_module() not found; "
                           "falling back to load_module()")
                    _import_warning(msg, 866)
                    spec.loader.load_module(name)
                else:
                    spec.loader.exec_module(module)
        finally:
            # Update the order of insertion into sys.modules for module
            # clean-up at shutdown.
            module = sys.modules.pop(spec.name)
            sys.modules[spec.name] = module
    return module



def _load_backward_compatible(spec):
    # It is assumed that all callers have been warned about using load_module()
    # appropriately before calling this function.
    try:
        spec.loader.load_module(spec.name)
    except:
        if spec.name in sys.modules:
            module = sys.modules.pop(spec.name)
            sys.modules[spec.name] = module
        raise
    # The module must be in sys.modules at this point!
    # Move it to the end of sys.modules.
    module = sys.modules.pop(spec.name)
    sys.modules[spec.name] = module
    if getattr(module, '__loader__', None) is None:
        try:
            module.__loader__ = spec.loader
        except AttributeError:
            pass
    if getattr(module, '__package__', None) is None:
        try:
            # Since module.__path__ may not line up with
            # spec.submodule_search_paths, we can't necessarily rely
            # on spec.parent here.
            module.__package__ = module.__name__
            if not hasattr(module, '__path__'):
                module.__package__ = spec.name.rpartition('.')[0]
        except AttributeError:
            pass
    if getattr(module, '__spec__', None) is None:
        try:
            module.__spec__ = spec
        except AttributeError:
            pass
    return module

def _load_unlocked(spec):
    # A helper for direct use by the import system.
    if spec.loader is not None:
        # Not a namespace package.
        if not hasattr(spec.loader, 'exec_module'):
            msg = (f"{_object_name(spec.loader)}.exec_module() not found; "
                    "falling back to load_module()")
            _import_warning(msg, 921)
            return _load_backward_compatible(spec)

    module = module_from_spec(spec)

    # This must be done before putting the module in sys.modules
    # (otherwise an optimization shortcut in import.c becomes
    # wrong).
    spec._initializing = True
    try:
        sys.modules[spec.name] = module
        try:
            if spec.loader is None:
                if spec.submodule_search_locations is None:
                    raise ImportError('missing loader', name=spec.name)
                # A namespace package so do nothing.
            else:
                spec.loader.exec_module(module)
        except:
            try:
                del sys.modules[spec.name]
            except KeyError:
                pass
            raise
        # Move the module to the end of sys.modules.
        # We don't ensure that the import-related module attributes get
        # set in the sys.modules replacement case.  Such modules are on
        # their own.
        module = sys.modules.pop(spec.name)
        sys.modules[spec.name] = module
        _verbose_message('import {!r} # {!r}', spec.name, spec.loader)
    finally:
        spec._initializing = False

    return module

# A method used during testing of _load_unlocked() and by
# _load_module_shim().
def _load(spec):
    """Return a new module object, loaded by the spec's loader.

    The module is not added to its parent.

    If a module is already in sys.modules, that existing module gets
    clobbered.

    """
    with _ModuleLockManager(spec.name):
        return _load_unlocked(spec)



def _gcd_import(name, package=None, level=0):
    """Import and return the module based on its name, the package the call is
    being made from, and the level adjustment.

    This function represents the greatest common denominator of functionality
    between import_module and __import__. This includes setting __package__ if
    the loader did not.

    """
    _sanity_check(name, package, level)
    if level > 0:
        name = _resolve_name(name, package, level)
    return _find_and_load(name, _gcd_import)


def _handle_fromlist(module, fromlist, import_, *, recursive=False):
    """Figure out what __import__ should return.

    The import_ parameter is a callable which takes the name of module to
    import. It is required to decouple the function from assuming importlib's
    import implementation is desired.

    """
    # The hell that is fromlist ...
    # If a package was imported, try to import stuff from fromlist.
    for x in fromlist:
        if not isinstance(x, str):
            if recursive:
                where = module.__name__ + '.__all__'
            else:
                where = "``from list''"
            raise TypeError(f"Item in {where} must be str, "
                            f"not {type(x).__name__}")
        elif x == '*':
            if not recursive and hasattr(module, '__all__'):
                _handle_fromlist(module, module.__all__, import_,
                                 recursive=True)
        elif not hasattr(module, x):
            from_name = f'{module.__name__}.{x}'
            try:
                _call_with_frames_removed(import_, from_name)
            except ModuleNotFoundError as exc:
                # Backwards-compatibility dictates we ignore failed
                # imports triggered by fromlist for modules that don't
                # exist.
                if (exc.name == from_name and
                    sys.modules.get(from_name, _NEEDS_LOADING) is not None):
                    continue
                raise
    return module


def _calc___package__(globals):
    """Calculate what __package__ should be.

    __package__ is not guaranteed to be defined or could be set to None
    to represent that its proper value is unknown.

    """
    package = globals.get('__package__')
    spec = globals.get('__spec__')
    if package is not None:
        if spec is not None and package != spec.parent:
            _warning_module().warn("__package__ != __spec__.parent "
                           f"({package!r} != {spec.parent!r})",
                           DeprecationWarning, stacklevel=3)
        return package
    elif spec is not None:
        return spec.parent
    else:
        _warning_module().warn("can't resolve package from __spec__ or __package__, "
                       "falling back on __name__ and __path__",
                       ImportWarning, stacklevel=3)
        package = globals['__name__']
        if '__path__' not in globals:
            package = package.rpartition('.')[0]
    return package


def __import__(name, globals=None, locals=None, fromlist=(), level=0):
    """Import a module.

    The 'globals' argument is used to infer where the import is occurring from
    to handle relative imports. The 'locals' argument is ignored. The
    'fromlist' argument specifies what should exist as attributes on the module
    being imported (e.g. ``from module import <fromlist>``).  The 'level'
    argument represents the package location to import from in a relative
    import (e.g. ``from ..pkg import mod`` would have a 'level' of 2).

    """
    if level == 0:
        module = _gcd_import(name)
    else:
        globals_ = globals if globals is not None else {}
        package = _calc___package__(globals_)
        module = _gcd_import(name, package, level)
    if not fromlist:
        # Return up to the first dot in 'name'. This is complicated by the fact
        # that 'name' may be relative.
        if level == 0:
            return _gcd_import(name.partition('.')[0])
        elif not name:
            return module
        else:
            # Figure out where to slice the module's name up to the first dot
            # in 'name'.
            cut_off = len(name) - len(name.partition('.')[0])
            # Slice end needs to be positive to alleviate need to special-case
            # when ``'.' not in name``.
            return sys.modules[module.__name__[:len(module.__name__)-cut_off]]
    elif hasattr(module, '__path__'):
        return _handle_fromlist(module, fromlist, _gcd_import)
    else:
        return module
