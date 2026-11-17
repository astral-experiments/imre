"""Discover modules in filesystem import roots."""

from collections import namedtuple
from itertools import count
from pathlib import Path
from typing import TYPE_CHECKING, Iterator

# Supplied by the checked import resolver.
_filesystem_roots: tuple[str, ...]
_module_suffixes: tuple[str, ...]

ModuleInfo = namedtuple('ModuleInfo', ('module_finder', 'name', 'ispkg'))

if TYPE_CHECKING:
    # The executable body is linked from pkgutil/boxed.py.
    def get_data(package: str, resource: str) -> bytes | None: ...


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
    paths = list(_filesystem_roots) if path is None else path
    seen: set[str] = set()
    for info in (
        item for directory in paths
        for item in _iter_file_modules(FileFinder(directory), prefix, seen)
    ):
        yield info
