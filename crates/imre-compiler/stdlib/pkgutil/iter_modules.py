class FileFinder:
    path: str

    def __init__(self, path: str) -> None:
        self.path = _absolute(path)


def _module_name(filename: str) -> str:
    for suffix in _module_suffixes:
        if filename.endswith(suffix):
            return filename[:-len(suffix)]
    return ''


def _is_package(directory: str) -> bool:
    if not _isdir(directory):
        return False
    for filename in _filenames(directory):
        if _module_name(filename) == '__init__':
            return True
    return False


def _module_info(finder: FileFinder, filename: str, prefix: str, seen: set[str]) -> ModuleInfo | None:
    name = _module_name(filename)
    ispkg = False
    if not name and '.' not in filename and _is_package(_join(finder.path, filename)):
        name = filename
        ispkg = True
    if not name or name == '__init__' or '.' in name or name in seen:
        return None
    seen.add(name)
    return ModuleInfo(finder, prefix + name, ispkg)
