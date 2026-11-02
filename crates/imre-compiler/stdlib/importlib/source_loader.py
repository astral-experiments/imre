"""Load Python modules from registered code or filesystem source."""

import sys

from _imre_importlib import (
    cache_name as _cache_name,
    get_code as _registered_code,
    loader_fields as _loader_fields,
    path_stats as _path_stats,
    source_name as _source_name,
    warn_debug_override as _warn_debug_override,
    warn_loader_deprecation as _warn_loader_deprecation,
)
from os import fspath as _fspath, sep


# Boxed source directories are searched directly; only ZIP path finders are cached.
_zip_path_importers = {}


def _invalidate_default_path_caches():
    if hasattr(sys, "path_hooks") or hasattr(sys, "path_importer_cache"):
        raise NotImplementedError("custom sys.path_hooks and sys.path_importer_cache")
    for path, finder in list(_zip_path_importers.items()):
        if finder is None or not path.startswith("/"):
            del _zip_path_importers[path]
        elif hasattr(finder, "invalidate_caches"):
            finder.invalidate_caches()


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
        _warn_loader_deprecation(globals(), False)
        spec_loader = loader

    if loader is not None and loader != spec_loader:
        _warn_loader_deprecation(globals(), True)
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
        _warn_debug_override()
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


def _get_cached(origin):
    # The standalone runtime has no bytecode cache tag.
    return origin if origin.endswith(".pyc") else None


class SourceFileLoader:
    def __init__(self, fullname, path):
        self.name = fullname
        self.path = path

    def __eq__(self, other):
        return self.__class__ == other.__class__ and _loader_fields(self) == _loader_fields(other)

    def __hash__(self):
        return hash(self.name) ^ hash(self.path)

    def create_module(self, spec):
        return None

    def load_module(self, name=None, *args, **kwargs):
        if name is None:
            name = self.name
        elif self.name != name:
            raise ImportError(f"loader for {self.name} cannot handle {name}", name=name)
        from importlib._legacy_source_loader import load
        return load(self, name, *args, **kwargs)

    def get_filename(self, fullname=None):
        if fullname is not None and fullname != self.name:
            raise ImportError(f"loader for {self.name} cannot handle {fullname}", name=fullname)
        return self.path

    def is_package(self, fullname):
        filename = self.get_filename(fullname).rpartition(sep)[2]
        return filename.rsplit(".", 1)[0] == "__init__" and fullname.rpartition(".")[2] != "__init__"

    def get_resource_reader(self, name=None):
        if name is not None and self.name != name:
            raise ImportError(f"loader for {self.name} cannot handle {name}", name=name)
        from importlib.resources._filesystem import FileReader
        return FileReader(self)

    def path_stats(self, path):
        return _path_stats(path)

    def get_data(self, path):
        with open(path, "rb") as file:
            return file.read()

    def source_to_code(self, data, path, *, _optimize=-1):
        from operator import index
        from os import fsdecode

        filename = fsdecode(path)
        if "\0" in filename:
            raise ValueError("embedded null character")
        level = index(_optimize)
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

    def exec_module(self, module):
        code = self.get_code(module.__name__)
        if code is None:
            raise ImportError(f"cannot load module {module.__name__!r} when get_code() returns None")
        exec(code, module.__dict__)
