from _imre_metadata import dedent, filenames as _filenames, zip_names as _zip_names, zip_open as _zip_open, zip_read as _zip_read
from os import fspath
from os.path import basename, isfile as _is_file, join as _join
import sys


class PackageNotFoundError(ModuleNotFoundError):
    def __str__(self):
        return f"No package metadata was found for {self.name}"

    @property
    def name(self):
        (name,) = self.args
        return name


def _normalize_separators(name):
    return '_'.join(re.split(r'[-_.]+', name))


def _basename(root):
    return basename(root.rstrip('/'))


class Distribution:
    @classmethod
    def from_name(cls, name):
        if not name:
            raise ValueError('A distribution name is required.')
        for dist in cls.discover(name=name):
            return dist
        raise PackageNotFoundError(name)

    @staticmethod
    def at(path):
        from pathlib import Path
        return PathDistribution(Path(path))

    @classmethod
    def discover(cls, *, context=None, **kwargs):
        if context and kwargs:
            raise ValueError('cannot accept context and kwargs')
        return _discover(context or _Context(**kwargs))

    @property
    def metadata(self):
        from importlib.metadata._adapters import Message
        text = self.read_text('METADATA') or self.read_text('PKG-INFO') or self.read_text('')
        return Message(text or '')

    @property
    def version(self):
        return self.metadata['Version']

    @property
    def name(self):
        return self.metadata['Name']

    @property
    def _normalized_name(self):
        return _normalize(self.name)

    @property
    def entry_points(self):
        return _dist_entry_points(self)


class PathDistribution(Distribution):
    def __init__(self, path):
        self._path = path

    def read_text(self, filename):
        try:
            return self._path.joinpath(filename).read_text(encoding='utf-8')
        except (FileNotFoundError, IsADirectoryError, NotADirectoryError, PermissionError, KeyError):
            return None

    def locate_file(self, path):
        return self._path.parent / path

    @property
    def _normalized_name(self):
        stem = basename(str(self._path))
        if stem.endswith(('.dist-info', '.egg-info')):
            name = stem.rsplit('.', 1)[0].split('-', 1)[0]
            if name:
                return _normalize(name)
        return _normalize(self.name)


class _ZipDistribution(PathDistribution):
    def __init__(self, archive, entry):
        super().__init__(entry)
        self._archive = archive

    def locate_file(self, path):
        raise NotImplementedError('distribution archive member paths')

    def read_text(self, filename):
        key = _join(self._path, fspath(filename)) if filename else self._path
        data = _zip_read(self._archive, key)
        if data is not None:
            return data.decode('utf-8').replace('\r\n', '\n').replace('\r', '\n')
        return None


class _Context:
    name = None

    def __init__(self, **kwargs):
        vars(self).update(kwargs)

    @property
    def path(self):
        return vars(self).get('path', sys.path)


def _discover(context):
    finders = getattr(sys, 'meta_path', None)
    if finders is not None:
        for finder in finders:
            resolver = getattr(finder, 'find_distributions', None)
            if resolver:
                yield from resolver(context)
        return
    name = context.name
    normalized = _normalize(name) if name else ''
    legacy = name.lower().replace('-', '_') if name else ''
    for root in context.path:
        directory = fspath(root) or '.'
        archive = None
        try:
            if _is_file(directory):
                archive = _zip_open(directory)
                entries = _archive_children(_zip_names(archive))
            else:
                entries = _filenames(directory)
        except Exception:
            # A sys.path entry can be unreadable, malformed or an unsupported ZIP.
            continue
        for entry in _matching_entries(directory, entries, normalized, legacy):
            if archive is not None:
                yield _ZipDistribution(archive, entry)
            else:
                yield Distribution.at(_join(directory, entry))


def _unique(dists):
    seen = set()
    for dist in dists:
        name = dist._normalized_name
        if name not in seen:
            seen.add(name)
            yield dist


def distribution(distribution_name):
    return Distribution.from_name(distribution_name)


def distributions(**kwargs):
    return Distribution.discover(**kwargs)


def metadata(distribution_name):
    return distribution(distribution_name).metadata


def version(distribution_name):
    return distribution(distribution_name).version
