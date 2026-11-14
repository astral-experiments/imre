from imre._zipimport import open_index as _open_index
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
            _indexes[path] = _open_index(path)
        except _BadZipFile:
            return False
    return True


def _read(path: str, key: str) -> bytes:
    return _indexes[path].read(key)


class zipimporter(_ZipResourceLoader):
    """Read stored or deflated ZIP resources."""
