from importlib.metadata import PackageNotFoundError
from pathlib import Path
from textwrap import dedent
from zipfile import ZipFile


def _paths() -> list[str]:
    return _search_path


def _normalize_separators(name: str) -> str:
    return re.sub(r'[-_.]+', '_', name)


def _filenames(root: str) -> list[str]:
    return [entry.name for entry in Path(root).iterdir()]


def _distribution_text(root: str, key: str, archive: ZipFile | None) -> str:
    if archive is not None:
        try:
            return archive.read(key).decode('utf-8').replace('\r\n', '\n').replace('\r', '\n')
        except KeyError:
            return ''
    path = _join(root, key)
    return _read_text(path) if _is_file(path) else ''


def _join(root: str, entry: str) -> str:
    return str(Path(root) / entry)


def _basename(root: str) -> str:
    return Path(root).name


def _is_file(path: str) -> bool:
    return Path(path).is_file()


def _read_text(path: str) -> str:
    return Path(path).read_text(encoding='utf-8')


def version(distribution_name: str) -> str:
    """Read the first matching installed distribution's Version in import-path order."""
    if not distribution_name:
        raise ValueError('A distribution name is required.')
    name = _normalize(distribution_name)
    legacy_name = distribution_name.lower().replace('-', '_')
    for root in _paths():
        directory = root or '.'
        archive: ZipFile | None = None
        try:
            if _is_file(directory):
                archive = ZipFile(directory)
                entries = _archive_children(archive.namelist())
            else:
                entries = _filenames(directory)
        except Exception:
            entries = []
        try:
            for entry in _matching_entries(directory, entries, name, legacy_name):
                for key in [_join(entry, 'METADATA'), _join(entry, 'PKG-INFO'), entry]:
                    text = _distribution_text(directory, key, archive)
                    if text:
                        value = _first_header(text, 'Version')
                        if value is not None:
                            return value
                        break
                raise NotImplementedError('distribution metadata without a Version header')
        finally:
            if archive is not None:
                archive.close()
    raise PackageNotFoundError(distribution_name)
