"""Read package resources from indexed ZIP archives on POSIX."""

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
    if not _is_file(path):
        raise ZipImportError('not a Zip file', path=path)
    return path, prefix


def _directory(path: str) -> str:
    try:
        valid = _index(path)
    except OSError:
        raise ZipImportError(f"can't open Zip file: {path!r}", path=path)
    if not valid:
        raise ZipImportError(f'not a Zip file: {path!r}', path=path)
    return path


class zipimporter:
    archive: str
    prefix: str
    _indexed_archive: str

    def __init__(self, path: str) -> None:
        parts = _archive_path(path)
        self.archive = parts[0]
        self.prefix = parts[1]
        self._indexed_archive = _directory(self.archive)

    def get_data(self, pathname: str) -> bytes:
        """Read a member by its archive-relative or archive-prefixed name."""
        if self.archive != self._indexed_archive:
            raise NotImplementedError('changing zipimporter.archive is unsupported')
        key = pathname
        if pathname.startswith(self.archive + '/'):
            key = pathname[len(self.archive) + 1:]
        return _read(self._indexed_archive, key)
