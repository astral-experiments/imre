from functools import partial as _partial
from importlib.metadata import PackageNotFoundError
from pathlib import Path
from textwrap import dedent
from zipfile import ZipFile


# Supplied by the checked import resolver.
_filesystem_roots: tuple[str, ...]


def _paths() -> tuple[str, ...]:
    return _filesystem_roots


def _filenames(root: str) -> list[str]:
    return [entry.name for entry in Path(root).iterdir()]


def _first_header(text: str, name: str) -> str | None:
    lines = text.split('\n')
    prefix = name.lower() + ':'
    for i, line in enumerate(lines):
        if not line:
            break
        if line.startswith(' ') or line.startswith('\t'):
            continue
        if re.search(r'^[!-9;-~]+:', line) is None:
            break
        if line[:len(prefix)].lower() == prefix:
            value = line[len(prefix):]
            for following in lines[i + 1:]:
                if not (following.startswith(' ') or following.startswith('\t')):
                    break
                value += '\n' + following
            value = value.lstrip(' \t\r\n')
            return dedent(' ' * 8 + value) if '\n' in value else value
    return None


def _zip_read(archive: ZipFile, key: str) -> bytes | None:
    try:
        return archive.read(key)
    except KeyError:
        return None


def _basename(root: str) -> str:
    return Path(root).name


def _is_file(path: str) -> bool:
    return Path(path).is_file()


def _resource_text(directory: str, entry: str, archive: ZipFile | None, filename: str) -> str | None:
    if archive is None:
        try:
            return _path_text(Path(directory) / entry, filename)
        except (FileNotFoundError, IsADirectoryError, NotADirectoryError, PermissionError, KeyError):
            return None
    key = str(Path(entry) / filename) if filename else entry
    return _zip_text(archive, key)


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
                text = _metadata_text(_partial(_resource_text, directory, entry, archive))
                if text:
                    value = _first_header(text, 'Version')
                    if value is not None:
                        return value
                raise NotImplementedError('distribution metadata without a Version header')
        finally:
            if archive is not None:
                archive.close()
    raise PackageNotFoundError(distribution_name)
