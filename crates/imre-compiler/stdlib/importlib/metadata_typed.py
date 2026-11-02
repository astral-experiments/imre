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


def _is_file(path: str) -> bool:
    return Path(path).is_file()


def _read_text(path: str) -> str:
    return Path(path).read_text(encoding='utf-8')


def version(distribution_name: str) -> str:
    """Read the first matching unpacked distribution's Version in import-path order."""
    if not distribution_name:
        raise ValueError('A distribution name is required.')
    name = _normalize(distribution_name)
    legacy_name = distribution_name.lower().replace('-', '_')
    for root in _paths():
        for path in _matching_paths(root, name, legacy_name):
            for metadata_path in [_join(path, 'METADATA'), _join(path, 'PKG-INFO'), path]:
                if _is_file(metadata_path):
                    text = _read_text(metadata_path)
                    if text:
                        value = _first_header(text, 'Version')
                        if value is not None:
                            return value
                        break
            raise NotImplementedError('distribution metadata without a Version header')
    raise PackageNotFoundError(distribution_name)
