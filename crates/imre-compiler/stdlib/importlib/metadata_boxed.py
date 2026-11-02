from _imre_metadata import dedent, filenames as _filenames
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

    def _header(self, key):
        text = self.read_text('METADATA') or self.read_text('PKG-INFO') or self.read_text('')
        value = _first_header(text or '', key)
        if value is None:
            import warnings
            warnings.warn('Implicit None on return values is deprecated and will raise KeyErrors.',
                          DeprecationWarning, stacklevel=2)
        return value

    @property
    def version(self):
        return self._header('Version')

    @property
    def name(self):
        return self._header('Name')

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
        stem = _basename(str(self._path))
        if stem.endswith(('.dist-info', '.egg-info')):
            name = stem.rsplit('.', 1)[0].split('-', 1)[0]
            if name:
                return _normalize(name)
        return _normalize(self.name)


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
        for path in _matching_paths(fspath(root), normalized, legacy):
            yield Distribution.at(path)


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


def version(distribution_name):
    return distribution(distribution_name).version
