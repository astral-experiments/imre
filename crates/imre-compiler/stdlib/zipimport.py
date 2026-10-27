"""Read package resources from indexed ZIP archives on POSIX."""

from pathlib import Path as _Path
from posixpath import exists as _exists
from zipfile import BadZipFile as _BadZipFile, ZipFile as _ZipFile


class ZipImportError(ImportError):
    pass


def _archive_path(path: str) -> tuple[str, str]:
    if not path:
        raise ZipImportError('archive path is empty', path=path)
    prefix = ''
    while not _exists(path):
        if not path:
            raise ZipImportError('not a Zip file', path=path)
        parts = path.rsplit('/', 1)
        tail = parts[-1]
        if tail:
            prefix = tail + '/' + prefix
        path = parts[0] if len(parts) == 2 else ''
    if not _Path(path).is_file():
        raise ZipImportError('not a Zip file', path=path)
    return path, prefix


_indexes: dict[str, _ZipFile] = {}


def _directory(path: str) -> _ZipFile:
    if path in _indexes:
        return _indexes[path]
    # ZipInfo records remain usable after the indexing descriptor is closed.
    try:
        with _ZipFile(path) as index:
            _indexes[path] = index
        return index
    except _BadZipFile:
        raise ZipImportError(f'not a Zip file: {path!r}', path=path)
    except OSError:
        raise ZipImportError(f"can't open Zip file: {path!r}", path=path)


class zipimporter:
    archive: str
    prefix: str
    _index: _ZipFile

    def __init__(self, path: str) -> None:
        parts = _archive_path(path)
        self.archive = parts[0]
        self.prefix = parts[1]
        self._index = _directory(parts[0])

    def get_data(self, pathname: str) -> bytes:
        """Read a member by its archive-relative or archive-prefixed name."""
        if self.archive != self._index.filename:
            raise NotImplementedError('changing zipimporter.archive is unsupported')
        key = pathname
        if pathname.startswith(self.archive + '/'):
            key = pathname[len(self.archive) + 1:]
        return self._index._read_import_data(key)
