"""Find installed distributions and read their metadata headers."""
import re


def _normalize(name: str) -> str:
    return _normalize_separators(name).lower()


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
