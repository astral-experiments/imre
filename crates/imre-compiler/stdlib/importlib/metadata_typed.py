from importlib.metadata import PackageNotFoundError
from pathlib import Path
from textwrap import dedent


def _paths() -> list[str]:
    return _search_path


def _normalize_separators(name: str) -> str:
    return re.sub(r'[-_.]+', '_', name)


def _filenames(root: str) -> list[str]:
    return [entry.name for entry in Path(root).iterdir()]


def _join(root: str, entry: str) -> str:
    return str(Path(root) / entry)


def _basename(root: str) -> str:
    return Path(root).name


def _exists(path: str) -> bool:
    return Path(path).exists()


def _is_file(path: str) -> bool:
    return Path(path).is_file()


def _read_text(path: str) -> str:
    return Path(path).read_text(encoding='utf-8')
