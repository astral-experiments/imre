from _imre_metadata import filenames as _filenames, zip_names as _zip_names, zip_open as _zip_open, zip_read as _zip_read
import abc
from importlib.abc import MetaPathFinder
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


def _basename(root):
    return basename(root.rstrip('/'))


class DeprecatedNonAbstract:
    def __new__(cls, *args, **kwargs):
        # Some native builtin bases have no exposed class dictionary.
        names = {
            name for base in cls.__mro__ for name in getattr(base, '__dict__', ())
        }
        abstract = {
            name for name in names
            if getattr(getattr(cls, name), '__isabstractmethod__', False)
        }
        if abstract:
            import warnings
            warnings.warn(
                f"Unimplemented abstract methods {abstract}",
                DeprecationWarning,
                stacklevel=2,
            )
        return super().__new__(cls)


class Distribution(DeprecatedNonAbstract):
    @abc.abstractmethod
    def read_text(self, filename):
        pass

    @abc.abstractmethod
    def locate_file(self, path):
        pass

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
        context = context or DistributionFinder.Context(**kwargs)
        resolvers = (getattr(finder, 'find_distributions', None) for finder in sys.meta_path)
        return _discover(context, resolvers)

    @property
    def metadata(self):
        from importlib.metadata._adapters import Message
        text = _metadata_text(lambda filename: self.read_text(filename))
        return Message(text or '')

    @property
    def version(self):
        return self.metadata['Version']

    @property
    def name(self):
        return self.metadata['Name']

    @property
    def files(self):
        from importlib.metadata._files import distribution_files
        return distribution_files(self)

    @property
    def origin(self):
        text = self.read_text('direct_url.json')
        if text is None:
            return None
        from json import loads
        from types import SimpleNamespace
        return loads(text, object_hook=lambda data: SimpleNamespace(**data))

    @property
    def _normalized_name(self):
        return _normalize(self.name)

    @property
    def entry_points(self):
        return _dist_entry_points(self)

    @property
    def requires(self):
        requirements = self.metadata.get_all('Requires-Dist')
        if requirements:
            return list(requirements)
        text = self.read_text('requires.txt')
        return None if text is None else list(self._deps_from_requires_text(text))

    @staticmethod
    def _deps_from_requires_text(text):
        # Setuptools stores extras and environment markers in section headers.
        marker = ''
        for raw_line in text.splitlines():
            line = raw_line.strip()
            if not line:
                continue
            if line.startswith('[') and line.endswith(']'):
                extra, _, condition = line.strip('[]').partition(':')
                if extra and condition:
                    condition = '(' + condition + ')'
                extra_marker = f'extra == "{extra}"' if extra else ''
                conditions = ' and '.join(filter(None, (condition, extra_marker)))
                marker = '; ' + conditions if conditions else ''
            else:
                # PEP 508 requires whitespace after a URL requirement.
                yield line + (' ' if '@' in line else '') + marker


class PathDistribution(Distribution):
    def __init__(self, path):
        self._path = path

    def read_text(self, filename):
        try:
            return _path_text(self._path, filename)
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
    def __init__(self, archive, filename, entry):
        super().__init__(entry)
        self._archive = archive
        self._filename = filename
        self._members = None

    def locate_file(self, path):
        from importlib.metadata._files import _ArchiveMember
        if self._members is None:
            # ZIP directories can be implied by their children.
            names = set(_zip_names(self._archive))
            for member in list(names):
                parent = member.rstrip('/').rpartition('/')[0]
                while parent and parent + '/' not in names:
                    names.add(parent + '/')
                    parent = parent.rpartition('/')[0]
            self._members = names
        return _ArchiveMember(self, _join('', path))

    def read_text(self, filename):
        key = _join(self._path, fspath(filename)) if filename else self._path
        return _zip_text(self._archive, key)


class DistributionFinder(MetaPathFinder):
    """A meta path finder that can discover installed distributions."""

    class Context:
        """Parameters supplied to distribution finders for one search."""

        name = None

        def __init__(self, **kwargs):
            vars(self).update(kwargs)

        @property
        def path(self):
            return vars(self).get('path', sys.path)

    @abc.abstractmethod
    def find_distributions(self, context=Context()):
        """Return the distributions that match the search context."""


def _discover(context, resolvers):
    for resolver in resolvers:
        if resolver:
            yield from resolver(context)


def _path_distributions(paths, normalized, legacy):
    for path in paths:
        directory = path or '.'
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
                yield _ZipDistribution(archive, directory, entry)
            else:
                yield Distribution.at(_join(directory, entry))


class MetadataPathFinder(DistributionFinder):
    @classmethod
    def find_distributions(cls, context=DistributionFinder.Context()):
        name = context.name
        paths = context.path
        normalized = _normalize(name) if name else ''
        legacy = name.lower().replace('-', '_') if name else ''
        # Capture the path iterator at lookup, leaving filesystem access lazy.
        return _path_distributions(map(fspath, paths), normalized, legacy)

    @staticmethod
    def invalidate_caches():
        # The directory and archive readers open a fresh index for each search.
        pass


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


def requires(distribution_name):
    return distribution(distribution_name).requires


def version(distribution_name):
    return distribution(distribution_name).version


def files(distribution_name):
    return distribution(distribution_name).files


def _get_toplevel_name(path):
    top, *rest = path.parts
    if rest:
        return top
    from inspect import getmodulename
    return getmodulename(path) or str(path)


def _top_level_inferred(paths):
    if paths is None:
        return ()
    if isinstance(paths, (str, bytes)):
        paths = (paths,)
    try:
        files = iter(paths)
    except TypeError:
        # A custom distribution may supply just one PackagePath.
        files = (paths,)
    names = set(map(_get_toplevel_name, files))
    return (name for name in names if '.' not in name)


def packages_distributions():
    """Map top-level import names to the distributions providing them."""
    result = {}
    for dist in distributions():
        names = (dist.read_text('top_level.txt') or '').split()
        if not names:
            names = _top_level_inferred(dist.files)
        for name in names:
            result.setdefault(name, []).append(dist.metadata['Name'])
    return result


def __getattr__(name):
    # Loading pathlib is only necessary for callers using installed file paths.
    if name in ('PackagePath', 'FileHash'):
        from importlib.metadata import _files
        return getattr(_files, name)
    raise AttributeError(f"module 'importlib.metadata' has no attribute {name!r}")
