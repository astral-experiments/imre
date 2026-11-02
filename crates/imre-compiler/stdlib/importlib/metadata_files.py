"""Installed distribution file lists from wheel and egg metadata."""
from io import BytesIO, TextIOWrapper
from os.path import basename, dirname, join
from pathlib import Path, PurePosixPath

from _imre_metadata import zip_read


class FileHash:
    __module__ = 'importlib.metadata'

    def __init__(self, spec):
        self.mode, _, self.value = spec.partition('=')

    def __repr__(self):
        return f'<FileHash mode: {self.mode} value: {self.value}>'


class PackagePath(PurePosixPath):
    __module__ = 'importlib.metadata'

    def locate(self):
        return self.dist.locate_file(self)

    def read_text(self, encoding='utf-8'):
        return self.locate().read_text(encoding=encoding)

    def read_binary(self):
        return self.locate().read_bytes()


def _manifest_lines(dist):
    record = dist.read_text('RECORD')
    if record:
        return record.splitlines()

    installed = dist.read_text('installed-files.txt')
    subdir = getattr(dist, '_path', None)
    if installed and subdir:
        root = dist.locate_file('')
        # pip paths are relative to the egg-info directory, including paths
        # to data files installed outside the distribution root.
        base = root.resolve()
        return ('"{}"'.format((subdir / name).resolve().relative_to(base, walk_up=True).as_posix())
                for name in installed.splitlines())

    sources = dist.read_text('SOURCES.txt')
    return sources and ('"{}"'.format(name) for name in sources.splitlines())


def distribution_files(dist):
    lines = _manifest_lines(dist)
    if lines is None:
        return None
    import csv

    def make_file(name, hash=None, size=None):
        file = PackagePath(name)
        file.hash = FileHash(hash) if hash else None
        file.size = int(size) if size else None
        file.dist = dist
        return file

    paths = (make_file(*row) for row in csv.reader(lines))
    return [path for path in paths if path.locate().exists()]


class _ArchiveMember:
    """A metadata SimplePath retaining the same open ZIP as its distribution."""

    def __init__(self, dist, at):
        self._dist = dist
        if at and at not in dist._members and at + '/' in dist._members:
            at += '/'
        self.at = at

    def __str__(self):
        return join(self._dist._filename, self.at)

    @property
    def name(self):
        return basename(self.at.rstrip('/') or self._dist._filename)

    @property
    def parent(self):
        if not self.at:
            return Path(self._dist._filename).parent
        parent = dirname(self.at.rstrip('/'))
        return _ArchiveMember(self._dist, parent + '/' if parent else '')

    def joinpath(self, *parts):
        return _ArchiveMember(self._dist, join(self.at, *parts))

    __truediv__ = joinpath

    def exists(self):
        return self.at in self._dist._members

    def is_dir(self):
        return not self.at or self.at.endswith('/')

    def read_bytes(self):
        if self.is_dir():
            raise IsADirectoryError(str(self))
        value = zip_read(self._dist._archive, self.at)
        if value is None:
            raise FileNotFoundError(str(self))
        return value

    def read_text(self, encoding=None):
        with TextIOWrapper(BytesIO(self.read_bytes()), encoding=encoding) as stream:
            return stream.read()
