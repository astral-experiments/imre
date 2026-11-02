"""Load Python modules from registered code or filesystem source."""

import _imp
import marshal
import sys

from _imre_importlib import (
    cache_name as _cache_name,
    get_code as _registered_code,
    index as _index,
    loader_fields as _loader_fields,
    path_stats as _path_stats,
    source_name as _source_name,
    warn_debug_override as _warn_debug_override,
    warn_loader_deprecation as _warn_loader_deprecation,
    warn_import as _warn_import,
)
from os import fspath as _fspath, sep


_MS_WINDOWS = sys.platform == "win32"
if _MS_WINDOWS:
    import nt as _os
    path_separators = "\\/"
else:
    import posix as _os
    path_separators = "/"
path_sep = path_separators[0]
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
    return _cache_name(path, tag, str(optimization), sys.pycache_prefix)


def source_from_cache(path):
    if sys.implementation.cache_tag is None:
        raise NotImplementedError("sys.implementation.cache_tag is None")
    return _source_name(_cache_path(path), sys.pycache_prefix)


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
        with open(path, "rb") as file:
            return file.read()

    @_check_name
    def get_resource_reader(self, module):
        from importlib.resources._filesystem import FileReader
        return FileReader(self)


class SourceFileLoader(FileLoader, _LoaderBasics):
    def path_stats(self, path):
        return _path_stats(path)

    def source_to_code(self, data, path, *, _optimize=-1):
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
        return compile(data, filename, "exec")

    def get_code(self, fullname):
        path = self.get_filename(fullname)
        code = _registered_code(self.name, path)
        if code is not None:
            return code
        source = self.get_data(path)
        return self.source_to_code(source, path)


def _file_loader_defaults(location=None):
    """Choose a default loader when a FileFinder factory returns None."""
    if location is not None and location.endswith(tuple(_imp.extension_suffixes())):
        raise NotImplementedError("default extension file loaders are unsupported")
    return (SourceFileLoader, SOURCE_SUFFIXES), (SourcelessFileLoader, BYTECODE_SUFFIXES)


_code_type = type(_file_loader_defaults.__code__)
