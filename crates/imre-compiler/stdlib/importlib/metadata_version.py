"""Version lookup in unpacked distribution metadata on the executable's import path."""
import re


def _normalize(name: str) -> str:
    return _normalize_separators(name).lower()


def _version(path: str) -> str:
    if not _is_file(path):
        raise NotImplementedError('distribution metadata without a METADATA file')
    lines = _read_text(path).split('\n')
    for i, line in enumerate(lines):
        if not line:
            break
        if line.startswith(' ') or line.startswith('\t'):
            continue
        if re.search(r'^[!-9;-~]+:', line) is None:
            break
        if line[:8].lower() == 'version:':
            value = line[8:]
            for following in lines[i + 1:]:
                if not (following.startswith(' ') or following.startswith('\t')):
                    break
                value += '\n' + following
            value = value.lstrip(' \t\r\n')
            return dedent(' ' * 8 + value) if '\n' in value else value
    raise NotImplementedError('distribution metadata without a Version header')


def version(distribution_name: str) -> str:
    """Read the first matching unpacked wheel's Version header in import-path order."""
    if not distribution_name:
        raise ValueError('A distribution name is required.')
    name = _normalize(distribution_name)
    for root in _paths():
        directory = root or '.'
        if _is_file(directory):
            raise NotImplementedError('distribution lookup in archive import roots')
        try:
            entries = _filenames(directory)
        except OSError:
            continue
        for entry in entries:
            filename = entry.lower()
            if filename.endswith('.dist-info') or filename.endswith('.egg-info'):
                stem = filename.rsplit('.', 1)[0].split('-', 1)[0]
                if _normalize(stem) == name:
                    if filename.endswith('.egg-info'):
                        raise NotImplementedError('legacy egg distribution metadata')
                    return _version(_join(_join(directory, entry), 'METADATA'))
        dirname = _basename(directory).lower()
        if dirname.endswith('.egg') and _exists(_join(directory, 'EGG-INFO')):
            if dirname[:-4].split('-', 1)[0] == distribution_name.lower().replace('-', '_'):
                raise NotImplementedError('legacy egg distribution metadata')
    raise PackageNotFoundError(distribution_name)
