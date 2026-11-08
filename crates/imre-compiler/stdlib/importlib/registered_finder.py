"""Combine registered native modules with the filesystem entry that contains them."""
from _imre_importlib import (
    _installed_source_members, _installed_stdlib, registered_path, registered_path_spec,
)


# Keep the identity installed in sys.meta_path when callers rebind the public export.
_registered_path_finder = PathFinder
_registered_find_spec = PathFinder.__dict__['find_spec']


def _installed_source_spec(fullname, registry_path, target):
    # A compiled package can have children in the installed archive. Only search
    # those children from their own virtual parent, preserving application shadows.
    parts = fullname.split('.')
    if not all(part.isidentifier() for part in parts):
        return None
    member = '/'.join(parts)
    if (member + '.py' not in _installed_source_members
            and member + '/__init__.py' not in _installed_source_members):
        return None
    parent = parts[:-1]
    if registry_path != _path_join('<imre stdlib>', *parent):
        return None
    if _installed_stdlib is not None and _path_isfile(_installed_stdlib):
        entry = _path_join(_installed_stdlib, *parent)
        return _registered_path_finder.find_spec(fullname, [entry], target)
    return None


class _RegisteredFileFinder(FileFinder):
    def __init__(self, path, registry_path):
        super().__init__(path, *_get_supported_file_loaders())
        # The compiler also registers virtual paths, such as '<imre stdlib>/unittest'.
        self._registry_path = registry_path

    def find_spec(self, fullname, target=None):
        registered = registered_path_spec(fullname, self._registry_path)
        # Relative registry roots live inside the executable, independent of the working directory.
        if not _path_isabs(self._registry_path):
            if registered is None:
                return _installed_source_spec(fullname, self._registry_path, target)
            return registered
        if (registered is not None and registered.loader is not None
                and registered.submodule_search_locations is not None):
            return registered
        on_disk = super().find_spec(fullname, target)
        if on_disk is not None and on_disk.loader is not None:
            # Keep the issued loader when FileFinder selects the same compiled source file.
            if (registered is not None and type(on_disk.loader) is SourceFileLoader
                    and type(on_disk.origin) is str and on_disk.origin == registered.origin):
                return registered
            return on_disk
        return registered if registered is not None else on_disk


_file_path_hook = FileFinder.path_hook(*_get_supported_file_loaders())


def _registered_path_hook(path):
    registry_path = registered_path(path)
    if registry_path is None:
        raise ImportError('not a registered module path', path=path)
    return _RegisteredFileFinder(path, registry_path)

def _set_bootstrap_module(_bootstrap_module):
    global _bootstrap
    _bootstrap = _bootstrap_module
