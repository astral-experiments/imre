"""Discover modules and read package resources in filesystem import roots."""

from collections import namedtuple
from itertools import count
from pathlib import Path
from typing import Iterator


class FileFinder:
    path: str

    def __init__(self, path: str) -> None:
        self.path = str(Path(path).absolute())


ModuleInfo = namedtuple('ModuleInfo', ('module_finder', 'name', 'ispkg'))


def _module_name(filename: str) -> str:
    for suffix in _module_suffixes:
        if filename.endswith(suffix):
            return filename[:-len(suffix)]
    return ''


def _filenames(directory: Path) -> list[str]:
    names: list[str] = []
    try:
        for entry in directory.iterdir():
            names.append(entry.name)
    except OSError:
        return []
    names.sort()
    return names


def _is_package(directory: Path) -> bool:
    if not directory.is_dir():
        return False
    for filename in _filenames(directory):
        if _module_name(filename) == '__init__':
            return True
    return False


def iter_modules(path: list[str] | str | None = None, prefix: str = '') -> Iterator[ModuleInfo]:
    """Yield unique modules and regular packages in filesystem import order."""
    if isinstance(path, str):
        raise ValueError('path must be None or list of paths to look for modules in')
    paths = _search_path if path is None else path
    if not paths:
        return
    seen: set[str] = set()
    root = 0
    finder = FileFinder(paths[root])
    names = _filenames(Path(finder.path))
    index = 0
    for _ in count():
        # A consumer can change the next directory between yields.
        if index >= len(names):
            root += 1
            if root >= len(paths):
                return
            finder = FileFinder(paths[root])
            names = _filenames(Path(finder.path))
            index = 0
            continue
        filename = names[index]
        index += 1
        name = _module_name(filename)
        ispkg = False
        if not name and '.' not in filename and _is_package(Path(finder.path) / filename):
            name = filename
            ispkg = True
        if not name or name == '__init__' or '.' in name or name in seen:
            continue
        seen.add(name)
        yield ModuleInfo(finder, prefix + name, ispkg)


def _has_module(directory: Path, stem: str) -> bool:
    for suffix in _module_suffixes:
        if (directory / (stem + suffix)).is_file():
            return True
    return False


def get_data(package: str, resource: str) -> bytes | None:
    """Read binary data beside a module in the executable's filesystem import roots."""
    if package.startswith('.'):
        raise ImportError(f'no package specified for {package!r} (required for relative module names)')
    components = package.split('.')
    roots = _search_path
    for index, component in enumerate(components):
        namespaces: list[str] = []
        for root in roots:
            if not component or '/' in component:
                break
            parent = Path(root)
            directory = parent / component
            if _has_module(directory, '__init__'):
                if index == len(components) - 1:
                    return (directory / resource.lstrip('/')).read_bytes()
                namespaces = [str(directory)]
                break
            if _has_module(parent, component):
                if index == len(components) - 1:
                    return (parent / resource.lstrip('/')).read_bytes()
                name = '.'.join(components[:index + 1])
                child = name + '.' + components[index + 1]
                if child == package:
                    message = f'__path__ attribute not found on {name!r} while trying to find {child!r}'
                    raise ModuleNotFoundError(message, name=child) from AttributeError(
                        f"module {name!r} has no attribute '__path__'"
                    )
                raise ModuleNotFoundError(f'No module named {child!r}; {name!r} is not a package', name=child)
            if directory.is_dir():
                namespaces.append(str(directory))
        if not namespaces and index < len(components) - 1:
            name = '.'.join(components[:index + 1])
            raise ModuleNotFoundError(f'No module named {name!r}', name=name)
        roots = namespaces
    return None
