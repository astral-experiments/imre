"""Discover modules and read package resources in filesystem import roots."""

from collections import namedtuple
from itertools import count
from pathlib import Path
from typing import Iterator

ModuleInfo = namedtuple('ModuleInfo', ('module_finder', 'name', 'ispkg'))


def _absolute(directory: str) -> str:
    return str(Path(directory).absolute())


def _join(directory: str, name: str) -> str:
    return str(Path(directory) / name)


def _isdir(directory: str) -> bool:
    return Path(directory).is_dir()


def _filenames(directory: str) -> list[str]:
    names: list[str] = []
    try:
        for entry in Path(directory).iterdir():
            names.append(entry.name)
    except OSError:
        return []
    names.sort()
    return names


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
    names = _filenames(finder.path)
    index = 0
    for _ in count():
        if index >= len(names):
            root += 1
            if root >= len(paths):
                return
            finder = FileFinder(paths[root])
            names = _filenames(finder.path)
            index = 0
            continue
        info = _module_info(finder, names[index], prefix, seen)
        index += 1
        if info is None:
            continue
        yield info


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
