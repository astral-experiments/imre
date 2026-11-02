"""Combine registered native modules with the filesystem entry that contains them."""
from _imre_importlib import registered_path, registered_path_spec


# Keep the identity installed in sys.meta_path when callers rebind the public export.
_registered_path_finder = PathFinder


class _RegisteredFileFinder(FileFinder):
    def __init__(self, path, registry_path):
        super().__init__(path, (SourceFileLoader, SOURCE_SUFFIXES))
        # The compiler also registers virtual paths, such as '<imre stdlib>/unittest'.
        self._registry_path = registry_path

    def find_spec(self, fullname, target=None):
        registered = registered_path_spec(fullname, self._registry_path)
        # Relative registry roots live inside the executable, independent of the working directory.
        if not _path_isabs(self._registry_path):
            return registered
        if (registered is not None and registered.loader is not None
                and registered.submodule_search_locations is not None):
            return registered
        on_disk = super().find_spec(fullname, target)
        if on_disk is not None and on_disk.loader is not None:
            return on_disk
        return registered if registered is not None else on_disk


_file_path_hook = FileFinder.path_hook((SourceFileLoader, SOURCE_SUFFIXES))


def _registered_path_hook(path):
    registry_path = registered_path(path)
    if registry_path is not None:
        return _RegisteredFileFinder(path, registry_path)
    return _file_path_hook(path)
