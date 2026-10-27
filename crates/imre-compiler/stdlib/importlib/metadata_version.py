"""Version lookup in unpacked distribution metadata on the executable's import path."""
import re
from importlib.metadata import PackageNotFoundError
from pathlib import Path
from textwrap import dedent


def _normalize(name: str) -> str:
    return re.sub(r'[-_.]+', '_', name).lower()


def _version(path: Path) -> str:
    if not path.is_file():
        raise NotImplementedError('distribution metadata without a METADATA file')
    lines = path.read_text(encoding='utf-8').split('\n')
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
    for root in _search_path:
        directory = Path(root)
        if directory.is_file():
            raise NotImplementedError('distribution lookup in archive import roots')
        try:
            entries = directory.iterdir()
        except OSError:
            continue
        for metadata in entries:
            filename = metadata.name.lower()
            if filename.endswith('.dist-info') or filename.endswith('.egg-info'):
                stem = filename.rsplit('.', 1)[0].split('-', 1)[0]
                if _normalize(stem) == name:
                    if filename.endswith('.egg-info'):
                        raise NotImplementedError('legacy egg distribution metadata')
                    return _version(metadata / 'METADATA')
        dirname = directory.name.lower()
        if dirname.endswith('.egg') and (directory / 'EGG-INFO').exists():
            if dirname[:-4].split('-', 1)[0] == distribution_name.lower().replace('-', '_'):
                raise NotImplementedError('legacy egg distribution metadata')
    raise PackageNotFoundError(distribution_name)
