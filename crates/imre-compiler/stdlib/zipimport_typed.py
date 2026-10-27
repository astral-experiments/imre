from pathlib import Path as _Path
from posixpath import exists as _exists
from zipfile import BadZipFile as _BadZipFile, ZipFile as _ZipFile

class ZipImportError(ImportError):
    pass


_indexes: dict[str, _ZipFile] = {}


def _is_file(path: str) -> bool:
    return _Path(path).is_file()


def _index(path: str) -> bool:
    if path not in _indexes:
        try:
            with _ZipFile(path) as index:
                _indexes[path] = index
        except _BadZipFile:
            return False
    return True


def _read(path: str, key: str) -> bytes:
    return _indexes[path]._read_import_data(key)
