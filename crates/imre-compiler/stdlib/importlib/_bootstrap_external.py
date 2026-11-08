"""Load Python modules from registered code or filesystem source."""

import _imp
import _io
import marshal
import sys

from _imre_importlib import (
    cache_name as _cache_name,
    get_code as _registered_code,
    get_source_data as _registered_source_data,
    index as _index,
    loader_fields as _loader_fields,
    path_stats as _path_stats,
    source_name as _source_name,
    warn_debug_override as _warn_debug_override,
    warn_loader_deprecation as _warn_loader_deprecation,
    warn_import as _warn_import,
)


_MS_WINDOWS = sys.platform == "win32"
if _MS_WINDOWS:
    import nt as _os
    path_separators = "\\/"
else:
    import posix as _os
    path_separators = "/"
try:
    _fspath = _os.fspath
except AttributeError:
    from os import fspath as _fspath
path_sep = sep = path_separators[0]
path_sep_tuple = tuple(path_separators)

SOURCE_SUFFIXES = [".py", ".pyw"] if _MS_WINDOWS else [".py"]
BYTECODE_SUFFIXES = [".pyc"]
MAGIC_NUMBER = _imp.pyc_magic_number_token.to_bytes(4, 'little')
EXTENSION_SUFFIXES = _imp.extension_suffixes()


_is_frozen = __name__ == "_frozen_importlib_external"
if _is_frozen:
    import _frozen_importlib as _bootstrap
else:
    # The package connects the selected core after this module has executed.
    _bootstrap = None


def _bless_my_loader(module_globals):
    """Choose the loader for a module's globals, honoring the legacy __loader__."""
    if not isinstance(module_globals, dict):
        return None
    missing = object()
    loader = module_globals.get("__loader__", None)
    spec = module_globals.get("__spec__", missing)
    if loader is None:
        if spec is missing:
            return None
        if spec is None:
            raise ValueError("Module globals is missing a __spec__.loader")

    spec_loader = getattr(spec, "loader", missing)
    if spec_loader in (missing, None):
        if loader is None:
            if spec_loader is missing:
                raise AttributeError("Module globals is missing a __spec__.loader")
            raise ValueError("Module globals is missing a __spec__.loader")
        _warn_loader_deprecation(globals(), False, _is_frozen)
        spec_loader = loader

    if loader is not None and loader != spec_loader:
        _warn_loader_deprecation(globals(), True, _is_frozen)
        return loader
    return spec_loader


def decode_source(source_bytes):
    """Decode Python source using its declared encoding and universal newlines."""
    import tokenize

    encoding, _ = tokenize.detect_encoding(_io.BytesIO(source_bytes).readline)
    decoder = _io.IncrementalNewlineDecoder(None, True)
    return decoder.decode(source_bytes.decode(encoding))


def _cache_path(path):
    path = _fspath(path)
    if isinstance(path, bytes):
        # Bootstrap's POSIX path split searches the path with a text separator.
        raise TypeError("argument should be integer or bytes-like object, not 'str'")
    return path


def cache_from_source(path, debug_override=None, *, optimization=None):
    if debug_override is not None:
        _warn_debug_override(globals(), _is_frozen)
        if optimization is not None:
            raise TypeError("debug_override or optimization must be set to None")
        optimization = "" if debug_override else 1
    path = _cache_path(path)
    tag = sys.implementation.cache_tag
    if tag is None:
        raise NotImplementedError("sys.implementation.cache_tag is None")
    if not isinstance(tag, str):
        raise TypeError(f"sequence item 2: expected str instance, {type(tag).__name__} found")
    if optimization is None:
        optimization = sys.flags.optimize or ""
    return _cache_name(path, tag, str(optimization), BYTECODE_SUFFIXES, sys, _path_join)


def source_from_cache(path):
    if sys.implementation.cache_tag is None:
        raise NotImplementedError("sys.implementation.cache_tag is None")
    return _source_name(_cache_path(path), sys.pycache_prefix, SOURCE_SUFFIXES, _path_join)


class _LoaderBasics:
    def is_package(self, fullname):
        filename = self.get_filename(fullname).rpartition(sep)[2]
        return filename.rsplit(".", 1)[0] == "__init__" and fullname.rpartition(".")[2] != "__init__"

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        code = self.get_code(module.__name__)
        if code is None:
            raise ImportError(f"cannot load module {module.__name__!r} when get_code() returns None")
        exec(code, module.__dict__)

    def load_module(self, fullname):
        return _bootstrap._load_module_shim(self, fullname)


def _check_name(method):
    def checked(self, name=None, *args, **kwargs):
        if name is None:
            name = self.name
        elif self.name != name:
            raise ImportError("loader for %s cannot handle %s" % (self.name, name), name=name)
        return method(self, name, *args, **kwargs)

    # The external module is initialized before the bootstrap namespace is complete.
    for attr in ("__module__", "__name__", "__qualname__", "__doc__"):
        if hasattr(method, attr):
            setattr(checked, attr, getattr(method, attr))
    checked.__dict__.update(method.__dict__)
    return checked


def _read_source_file(loader, filename):
    data = _registered_source_data(loader, filename)
    if data is not None:
        return data
    with _io.open_code(filename) as file:
        return file.read()


class FileLoader:
    def __init__(self, fullname, path):
        self.name = fullname
        self.path = path

    def __eq__(self, other):
        return self.__class__ == other.__class__ and _loader_fields(self) == _loader_fields(other)

    def __hash__(self):
        return hash(self.name) ^ hash(self.path)

    @_check_name
    def load_module(self, fullname):
        return super(FileLoader, self).load_module(fullname)

    @_check_name
    def get_filename(self, fullname):
        return self.path

    def get_data(self, path):
        if isinstance(self, SourceFileLoader):
            return _read_source_file(self, str(path))
        with open(path, "rb") as file:
            return file.read()

    @_check_name
    def get_resource_reader(self, module):
        from importlib.readers import FileReader
        return FileReader(self)


def _compile_source(data, path, _optimize, registration=None):
    filename = _fspath(path)
    if isinstance(filename, bytes):
        filename = filename.decode(sys.getfilesystemencoding(), sys.getfilesystemencodeerrors())
    if "\0" in filename:
        raise ValueError("embedded null character")
    level = _index(_optimize)
    if level < -2147483648 or level > 2147483647:
        raise OverflowError("Python int too large to convert to C int")
    if level < -1 or level > 2:
        raise ValueError("compile(): invalid optimize value")
    if level == -1:
        level = sys.flags.optimize
    if level:
        raise NotImplementedError("native source compilation supports optimization level 0")
    if registration is not None:
        loader, fullname, read_name = registration
        if type(read_name) is str and type(filename) is str and read_name == filename:
            code = _registered_code(loader, filename, fullname, _file_data, _source_compiler)
            if code is not None:
                return code
    return compile(data, filename, "exec")


def _is_bound(method, instance, function):
    return type(method) is _method_type and method.__self__ is instance and method.__func__ is function


class SourceLoader(_LoaderBasics):
    def get_source(self, fullname):
        path = self.get_filename(fullname)
        try:
            data = self.get_data(path)
        except OSError as exc:
            raise ImportError("source not available through get_data()", name=fullname) from exc
        return decode_source(data)

    def source_to_code(self, data, path, *, _optimize=-1):
        return _compile_source(data, path, _optimize)

    def get_code(self, fullname):
        path = self.get_filename(fullname)
        if not isinstance(path, str):
            _cache_path(path)
            read = self.get_data
            if issubclass(type(self), SourceFileLoader) and _is_bound(read, self, _file_data):
                # Preserve the read before selecting the converter or asking for its filename.
                read_name = str(path)
                data = _read_source_file(self, read_name)
                convert = self.source_to_code
                if _is_bound(convert, self, _source_compiler):
                    return _compile_source(data, path, -1, (self, fullname, read_name))
                return convert(data, path)
            data = read(path)
            return self.source_to_code(data, path)
        if issubclass(type(self), SourceFileLoader):
            code = _registered_code(self, path, fullname, _file_data, _source_compiler)
            if code is not None:
                return code
        data = self.get_data(path)
        return self.source_to_code(data, path)


_file_data = FileLoader.get_data
_method_type = type(_file_data.__get__(FileLoader))
_source_compiler = SourceLoader.source_to_code


class SourceFileLoader(FileLoader, SourceLoader):
    def path_stats(self, path):
        return _path_stats(path)


def _file_loader_defaults(location=None):
    """Choose a default loader when a FileFinder factory returns None."""
    if location is not None and location.endswith(tuple(_imp.extension_suffixes())):
        raise NotImplementedError("default extension file loaders are unsupported")
    return (SourceFileLoader, SOURCE_SUFFIXES), (SourcelessFileLoader, BYTECODE_SUFFIXES)


_code_type = type(_file_loader_defaults.__code__)

_CASE_INSENSITIVE_PLATFORMS_STR_KEY = 'win',
_CASE_INSENSITIVE_PLATFORMS_BYTES_KEY = 'cygwin', 'darwin', 'ios', 'tvos', 'watchos'
_CASE_INSENSITIVE_PLATFORMS =  (_CASE_INSENSITIVE_PLATFORMS_BYTES_KEY
                                + _CASE_INSENSITIVE_PLATFORMS_STR_KEY)


def _make_relax_case():
    if sys.platform.startswith(_CASE_INSENSITIVE_PLATFORMS):
        if sys.platform.startswith(_CASE_INSENSITIVE_PLATFORMS_STR_KEY):
            key = 'PYTHONCASEOK'
        else:
            key = b'PYTHONCASEOK'

        def _relax_case():
            """True if filenames must be checked case-insensitively and ignore environment flags are not set."""
            return not sys.flags.ignore_environment and key in _os.environ
    else:
        def _relax_case():
            """True if filenames must be checked case-insensitively."""
            return False
    return _relax_case

_relax_case = _make_relax_case()


def _pack_uint32(x):
    """Convert a 32-bit integer to little-endian."""
    return (int(x) & 0xFFFFFFFF).to_bytes(4, 'little')


def _unpack_uint64(data):
    """Convert 8 bytes in little-endian to an integer."""
    assert len(data) == 8
    return int.from_bytes(data, 'little')

def _unpack_uint32(data):
    """Convert 4 bytes in little-endian to an integer."""
    assert len(data) == 4
    return int.from_bytes(data, 'little')


if _MS_WINDOWS:
    def _path_join(*path_parts):
        """Replacement for os.path.join()."""
        if not path_parts:
            return ""
        if len(path_parts) == 1:
            return path_parts[0]
        root = ""
        path = []
        for new_root, tail in map(_os._path_splitroot, path_parts):
            if new_root.startswith(path_sep_tuple) or new_root.endswith(path_sep_tuple):
                root = new_root.rstrip(path_separators) or root
                path = [path_sep + tail]
            elif new_root.endswith(':'):
                if root.casefold() != new_root.casefold():
                    # Drive relative paths have to be resolved by the OS, so we reset the
                    # tail but do not add a path_sep prefix.
                    root = new_root
                    path = [tail]
                else:
                    path.append(tail)
            else:
                root = new_root or root
                path.append(tail)
        path = [p.rstrip(path_separators) for p in path if p]
        if len(path) == 1 and not path[0]:
            # Avoid losing the root's trailing separator when joining with nothing
            return root + path_sep
        return root + path_sep.join(path)

else:
    def _path_join(*path_parts):
        """Replacement for os.path.join()."""
        return path_sep.join([part.rstrip(path_separators)
                              for part in path_parts if part])


def _path_split(path):
    """Replacement for os.path.split()."""
    i = max(path.rfind(p) for p in path_separators)
    if i < 0:
        return '', path
    return path[:i], path[i + 1:]


def _path_stat(path):
    """Stat the path.

    Made a separate function to make it easier to override in experiments
    (e.g. cache stat results).

    """
    return _os.stat(path)


def _path_is_mode_type(path, mode):
    """Test whether the path is the specified mode type."""
    try:
        stat_info = _path_stat(path)
    except OSError:
        return False
    return (stat_info.st_mode & 0o170000) == mode


def _path_isfile(path):
    """Replacement for os.path.isfile."""
    return _path_is_mode_type(path, 0o100000)


def _path_isdir(path):
    """Replacement for os.path.isdir."""
    if not path:
        path = _os.getcwd()
    return _path_is_mode_type(path, 0o040000)


if _MS_WINDOWS:
    def _path_isabs(path):
        """Replacement for os.path.isabs."""
        if not path:
            return False
        root = _os._path_splitroot(path)[0].replace('/', '\\')
        return len(root) > 1 and (root.startswith('\\\\') or root.endswith('\\'))

else:
    def _path_isabs(path):
        """Replacement for os.path.isabs."""
        return path.startswith(path_separators)


def _path_abspath(path):
    """Replacement for os.path.abspath."""
    if not _path_isabs(path):
        for sep in path_separators:
            path = path.removeprefix(f".{sep}")
        return _path_join(_os.getcwd(), path)
    else:
        return path



def _get_cached(filename):
    if filename.endswith(tuple(SOURCE_SUFFIXES)):
        try:
            return cache_from_source(filename)
        except NotImplementedError:
            pass
    elif filename.endswith(tuple(BYTECODE_SUFFIXES)):
        return filename
    else:
        return None



def _classify_pyc(data, name, exc_details):
    """Perform basic validity checking of a pyc header and return the flags field,
    which determines how the pyc should be further validated against the source.

    *data* is the contents of the pyc file. (Only the first 16 bytes are
    required, though.)

    *name* is the name of the module being imported. It is used for logging.

    *exc_details* is a dictionary passed to ImportError if it raised for
    improved debugging.

    ImportError is raised when the magic number is incorrect or when the flags
    field is invalid. EOFError is raised when the data is found to be truncated.

    """
    magic = data[:4]
    if magic != MAGIC_NUMBER:
        message = f'bad magic number in {name!r}: {magic!r}'
        _bootstrap._verbose_message('{}', message)
        raise ImportError(message, **exc_details)
    if len(data) < 16:
        message = f'reached EOF while reading pyc header of {name!r}'
        _bootstrap._verbose_message('{}', message)
        raise EOFError(message)
    flags = _unpack_uint32(data[4:8])
    # Only the first two flags are defined.
    if flags & ~0b11:
        message = f'invalid flags {flags!r} in {name!r}'
        raise ImportError(message, **exc_details)
    return flags



def _compile_bytecode(data, name=None, bytecode_path=None, source_path=None):
    """Compile bytecode as found in a pyc."""
    code = marshal.loads(data)
    if isinstance(code, _code_type):
        _bootstrap._verbose_message('code object from {!r}', bytecode_path)
        if source_path is not None:
            _imp._fix_co_filename(code, source_path)
        return code
    else:
        raise ImportError(f'Non-code object in {bytecode_path!r}',
                          name=name, path=bytecode_path)



class SourcelessFileLoader(FileLoader, _LoaderBasics):

    """Loader which handles sourceless file imports."""

    def get_code(self, fullname):
        path = self.get_filename(fullname)
        data = self.get_data(path)
        # Call _classify_pyc to do basic validation of the pyc but ignore the
        # result. There's no source to check against.
        exc_details = {
            'name': fullname,
            'path': path,
        }
        _classify_pyc(data, fullname, exc_details)
        return _compile_bytecode(
            memoryview(data)[16:],
            name=fullname,
            bytecode_path=path,
        )

    def get_source(self, fullname):
        """Return None as there is no source code."""
        return None



_POPULATE = object()


def spec_from_file_location(name, location=None, *, loader=None,
                            submodule_search_locations=_POPULATE):
    """Return a module spec based on a file location.

    To indicate that the module is a package, set
    submodule_search_locations to a list of directory paths.  An
    empty list is sufficient, though its not otherwise useful to the
    import system.

    The loader must take a spec as its only __init__() arg.

    """
    if location is None:
        # The caller may simply want a partially populated location-
        # oriented spec.  So we set the location to a bogus value and
        # fill in as much as we can.
        location = '<unknown>'
        if hasattr(loader, 'get_filename'):
            # ExecutionLoader
            try:
                location = loader.get_filename(name)
            except ImportError:
                pass
    else:
        location = _os.fspath(location)
        try:
            location = _path_abspath(location)
        except OSError:
            pass

    # If the location is on the filesystem, but doesn't actually exist,
    # we could return None here, indicating that the location is not
    # valid.  However, we don't have a good way of testing since an
    # indirect location (e.g. a zip file or URL) will look like a
    # non-existent file relative to the filesystem.

    spec = _bootstrap.ModuleSpec(name, loader, origin=location)
    spec._set_fileattr = True

    # Pick a loader if one wasn't provided.
    if loader is None:
        for loader_class, suffixes in _file_loader_defaults(location):
            if location.endswith(tuple(suffixes)):
                loader = loader_class(name, location)
                spec.loader = loader
                break
        else:
            return None

    # Set submodule_search_paths appropriately.
    if submodule_search_locations is _POPULATE:
        # Check the loader.
        if hasattr(loader, 'is_package'):
            try:
                is_package = loader.is_package(name)
            except ImportError:
                pass
            else:
                if is_package:
                    spec.submodule_search_locations = []
    else:
        spec.submodule_search_locations = submodule_search_locations
    if spec.submodule_search_locations == []:
        if location:
            dirname = _path_split(location)[0]
            spec.submodule_search_locations.append(dirname)

    return spec



class _NamespacePath:
    """Represents a namespace package's path.  It uses the module name
    to find its parent module, and from there it looks up the parent's
    __path__.  When this changes, the module's own path is recomputed,
    using path_finder.  For top-level modules, the parent module's path
    is sys.path."""

    # When invalidate_caches() is called, this epoch is incremented
    # https://bugs.python.org/issue45703
    _epoch = 0

    def __init__(self, name, path, path_finder):
        self._name = name
        self._path = path
        self._last_parent_path = tuple(self._get_parent_path())
        self._last_epoch = self._epoch
        self._path_finder = path_finder

    def _find_parent_path_names(self):
        """Returns a tuple of (parent-module-name, parent-path-attr-name)"""
        parent, dot, me = self._name.rpartition('.')
        if dot == '':
            # This is a top-level module. sys.path contains the parent path.
            return 'sys', 'path'
        # Not a top-level module. parent-module.__path__ contains the
        #  parent path.
        return parent, '__path__'

    def _get_parent_path(self):
        parent_module_name, path_attr_name = self._find_parent_path_names()
        return getattr(sys.modules[parent_module_name], path_attr_name)

    def _recalculate(self):
        # If the parent's path has changed, recalculate _path
        parent_path = tuple(self._get_parent_path()) # Make a copy
        if parent_path != self._last_parent_path or self._epoch != self._last_epoch:
            spec = self._path_finder(self._name, parent_path)
            # Note that no changes are made if a loader is returned, but we
            #  do remember the new parent path
            if spec is not None and spec.loader is None:
                if spec.submodule_search_locations:
                    self._path = spec.submodule_search_locations
            self._last_parent_path = parent_path     # Save the copy
            self._last_epoch = self._epoch
        return self._path

    def __iter__(self):
        return iter(self._recalculate())

    def __getitem__(self, index):
        return self._recalculate()[index]

    def __setitem__(self, index, path):
        self._path[index] = path

    def __len__(self):
        return len(self._recalculate())

    def __repr__(self):
        return f'_NamespacePath({self._path!r})'

    def __contains__(self, item):
        return item in self._recalculate()

    def append(self, item):
        self._path.append(item)


# This class is actually exposed publicly in a namespace package's __loader__
# attribute, so it should be available through a non-private name.
# https://github.com/python/cpython/issues/92054
class NamespaceLoader:
    def __init__(self, name, path, path_finder):
        self._path = _NamespacePath(name, path, path_finder)

    def is_package(self, fullname):
        return True

    def get_source(self, fullname):
        return ''

    def get_code(self, fullname):
        return compile('', '<string>', 'exec', dont_inherit=True)

    def create_module(self, spec):
        """Use default semantics for module creation."""

    def exec_module(self, module):
        pass

    def load_module(self, fullname):
        """Load a namespace module.

        This method is deprecated.  Use exec_module() instead.

        """
        # The import system never calls this method.
        _bootstrap._verbose_message('namespace module loaded with path {!r}',
                                    self._path)
        # Warning implemented in _load_module_shim().
        return _bootstrap._load_module_shim(self, fullname)

    def get_resource_reader(self, module):
        from importlib.readers import NamespaceReader
        return NamespaceReader(self._path)


# We use this exclusively in module_from_spec() for backward-compatibility.
_NamespaceLoader = NamespaceLoader


# Finders #####################################################################

class PathFinder:

    """Meta path finder for sys.path and package __path__ attributes."""

    @staticmethod
    def invalidate_caches():
        """Call the invalidate_caches() method on all path entry finders
        stored in sys.path_importer_cache (where implemented)."""
        for name, finder in list(sys.path_importer_cache.items()):
            # Drop entry if finder name is a relative path. The current
            # working directory may have changed.
            if finder is None or not _path_isabs(name):
                del sys.path_importer_cache[name]
            elif hasattr(finder, 'invalidate_caches'):
                finder.invalidate_caches()
        # Also invalidate the caches of _NamespacePaths
        # https://bugs.python.org/issue45703
        _NamespacePath._epoch += 1

        from importlib.metadata import MetadataPathFinder
        MetadataPathFinder.invalidate_caches()

    @staticmethod
    def _path_hooks(path):
        """Search sys.path_hooks for a finder for 'path'."""
        if sys.path_hooks is not None and not sys.path_hooks:
            _warn_import(globals(), 'sys.path_hooks is empty', 1224, _is_frozen)
        for hook in sys.path_hooks:
            try:
                return hook(path)
            except ImportError:
                continue
        else:
            return None

    @classmethod
    def _path_importer_cache(cls, path):
        """Get the finder for the path entry from sys.path_importer_cache.

        If the path entry is not in the cache, find the appropriate finder
        and cache it. If no finder is available, store None.

        """
        if path == '':
            try:
                path = _os.getcwd()
            except (FileNotFoundError, PermissionError):
                # Don't cache the failure as the cwd can easily change to
                # a valid directory later on.
                return None
        try:
            finder = sys.path_importer_cache[path]
        except KeyError:
            finder = cls._path_hooks(path)
            sys.path_importer_cache[path] = finder
        return finder

    @classmethod
    def _get_spec(cls, fullname, path, target=None):
        """Find the loader or namespace_path for this module/package name."""
        # If this ends up being a namespace package, namespace_path is
        #  the list of paths that will become its __path__
        namespace_path = []
        for entry in path:
            if not isinstance(entry, str):
                continue
            finder = cls._path_importer_cache(entry)
            if finder is not None:
                spec = finder.find_spec(fullname, target)
                if spec is None:
                    continue
                if spec.loader is not None:
                    return spec
                portions = spec.submodule_search_locations
                if portions is None:
                    raise ImportError('spec missing loader')
                # This is possibly part of a namespace package.
                #  Remember these path entries (if any) for when we
                #  create a namespace package, and continue iterating
                #  on path.
                namespace_path.extend(portions)
        else:
            spec = _bootstrap.ModuleSpec(fullname, None)
            spec.submodule_search_locations = namespace_path
            return spec

    @classmethod
    def find_spec(cls, fullname, path=None, target=None):
        """Try to find a spec for 'fullname' on sys.path or 'path'.

        The search is based on sys.path_hooks and sys.path_importer_cache.
        """
        if path is None:
            path = sys.path
        spec = cls._get_spec(fullname, path, target)
        if spec is None:
            return None
        elif spec.loader is None:
            namespace_path = spec.submodule_search_locations
            if namespace_path:
                # We found at least one namespace path.  Return a spec which
                # can create the namespace package.
                spec.origin = None
                spec.submodule_search_locations = _NamespacePath(fullname, namespace_path, cls._get_spec)
                return spec
            else:
                return None
        else:
            return spec

    @staticmethod
    def find_distributions(*args, **kwargs):
        """
        Find distributions.

        Return an iterable of all Distribution instances capable of
        loading the metadata for packages matching ``context.name``
        (or all names if ``None`` indicated) along the paths in the list
        of directories ``context.path``.
        """
        from importlib.metadata import MetadataPathFinder
        return MetadataPathFinder.find_distributions(*args, **kwargs)



class FileFinder:

    """File-based finder.

    Interactions with the file system are cached for performance, being
    refreshed when the directory the finder is handling has been modified.

    """

    def __init__(self, path, *loader_details):
        """Initialize with the path to search on and a variable number of
        2-tuples containing the loader and the file suffixes the loader
        recognizes."""
        loaders = []
        for loader, suffixes in loader_details:
            loaders.extend((suffix, loader) for suffix in suffixes)
        self._loaders = loaders
        # Base (directory) path
        if not path or path == '.':
            self.path = _os.getcwd()
        else:
            self.path = _path_abspath(path)
        self._path_mtime = -1
        self._path_cache = set()
        self._relaxed_path_cache = set()

    def invalidate_caches(self):
        """Invalidate the directory mtime."""
        self._path_mtime = -1

    def _get_spec(self, loader_class, fullname, path, smsl, target):
        loader = loader_class(fullname, path)
        return spec_from_file_location(fullname, path, loader=loader,
                                       submodule_search_locations=smsl)

    def find_spec(self, fullname, target=None):
        """Try to find a spec for the specified module.

        Returns the matching spec, or None if not found.
        """
        is_namespace = False
        tail_module = fullname.rpartition('.')[2]
        try:
            mtime = _path_stat(self.path or _os.getcwd()).st_mtime
        except OSError:
            mtime = -1
        if mtime != self._path_mtime:
            self._fill_cache()
            self._path_mtime = mtime
        # tail_module keeps the original casing, for __file__ and friends
        if _relax_case():
            cache = self._relaxed_path_cache
            cache_module = tail_module.lower()
        else:
            cache = self._path_cache
            cache_module = tail_module
        # Check if the module is the name of a directory (and thus a package).
        if cache_module in cache:
            base_path = _path_join(self.path, tail_module)
            for suffix, loader_class in self._loaders:
                init_filename = '__init__' + suffix
                full_path = _path_join(base_path, init_filename)
                if _path_isfile(full_path):
                    return self._get_spec(loader_class, fullname, full_path, [base_path], target)
            else:
                # If a namespace package, return the path if we don't
                #  find a module in the next section.
                is_namespace = _path_isdir(base_path)
        # Check for a file w/ a proper suffix exists.
        for suffix, loader_class in self._loaders:
            try:
                full_path = _path_join(self.path, tail_module + suffix)
            except ValueError:
                return None
            _bootstrap._verbose_message('trying {}', full_path, verbosity=2)
            if cache_module + suffix in cache:
                if _path_isfile(full_path):
                    return self._get_spec(loader_class, fullname, full_path,
                                          None, target)
        if is_namespace:
            _bootstrap._verbose_message('possible namespace for {}', base_path)
            spec = _bootstrap.ModuleSpec(fullname, None)
            spec.submodule_search_locations = [base_path]
            return spec
        return None

    def _fill_cache(self):
        """Fill the cache of potential modules and packages for this directory."""
        path = self.path
        try:
            contents = _os.listdir(path or _os.getcwd())
        except (FileNotFoundError, PermissionError, NotADirectoryError):
            # Directory has either been removed, turned into a file, or made
            # unreadable.
            contents = []
        # We store two cached versions, to handle runtime changes of the
        # PYTHONCASEOK environment variable.
        if not sys.platform.startswith('win'):
            self._path_cache = set(contents)
        else:
            # Windows users can import modules with case-insensitive file
            # suffixes (for legacy reasons). Make the suffix lowercase here
            # so it's done once instead of for every import. This is safe as
            # the specified suffixes to check against are always specified in a
            # case-sensitive manner.
            lower_suffix_contents = set()
            for item in contents:
                name, dot, suffix = item.partition('.')
                if dot:
                    new_name = f'{name}.{suffix.lower()}'
                else:
                    new_name = name
                lower_suffix_contents.add(new_name)
            self._path_cache = lower_suffix_contents
        if sys.platform.startswith(_CASE_INSENSITIVE_PLATFORMS):
            self._relaxed_path_cache = {fn.lower() for fn in contents}

    @classmethod
    def path_hook(cls, *loader_details):
        """A class method which returns a closure to use on sys.path_hook
        which will return an instance using the specified loaders and the path
        called on the closure.

        If the path called on the closure is not a directory, ImportError is
        raised.

        """
        def path_hook_for_FileFinder(path):
            """Path hook for importlib.machinery.FileFinder."""
            if not _path_isdir(path):
                raise ImportError('only directories are supported', path=path)
            return cls(path, *loader_details)

        return path_hook_for_FileFinder

    def __repr__(self):
        return f'FileFinder({self.path!r})'
