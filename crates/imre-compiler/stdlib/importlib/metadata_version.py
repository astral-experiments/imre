"""Find installed distributions and read their metadata headers."""
import re
from typing import Callable as _Callable


def _metadata_text(read_text: _Callable[[str], str | None]):
    return read_text('METADATA') or read_text('PKG-INFO') or read_text('')


def _path_text(path, filename):
    return path.joinpath(filename).read_text(encoding='utf-8')


def _zip_text(archive, key: str):
    data = _zip_read(archive, key)
    if data is None:
        return None
    return data.decode('utf-8').replace('\r\n', '\n').replace('\r', '\n')


def _normalize_separators(name: str) -> str:
    return re.sub(r'[-_.]+', '_', name)


def _normalize(name: str) -> str:
    return _normalize_separators(name).lower()


def _archive_children(members: list[str]) -> list[str]:
    directories: dict[str, bool] = {}
    for member in members:
        child = member.split('/', 1)[0]
        if child == member:
            directories[child] = False
        elif child not in directories:
            directories[child] = True
    # zipfile.Path represents implicit directories with a trailing slash; exact files win.
    return [child + '/' if is_dir else child for child, is_dir in directories.items()]


def _matching_entries(directory: str, entries: list[str], name: str, legacy_name: str) -> list[str]:
    matches: list[str] = []
    dirname = _basename(directory).lower()
    egg_info = ''
    for entry in entries:
        filename = entry.rstrip('/').lower()
        if filename.endswith('.dist-info') or filename.endswith('.egg-info'):
            stem = filename.rsplit('.', 1)[0].split('-', 1)[0]
            if not name or _normalize(stem) == name:
                matches.append(entry)
        elif filename == 'egg-info' and dirname.endswith('.egg'):
            stem = dirname[:-4].split('-', 1)[0].replace('-', '_')
            if not legacy_name or stem == legacy_name:
                egg_info = entry
    if egg_info:
        matches.append(egg_info)
    return matches


def _read_root(directory):
    """Open one import root and list its immediate distribution candidates."""
    archive: ZipFile | None = None
    entries: list[str] | None = None
    try:
        if _is_file(directory):
            archive = _zip_open(directory)
            entries = _archive_children(_zip_names(archive))
        else:
            entries = _filenames(directory)
    except Exception:
        pass
    return archive, entries
