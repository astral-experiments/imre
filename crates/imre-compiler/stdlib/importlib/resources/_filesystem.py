"""Resource reader for modules loaded from the local filesystem."""

import os


class _FilePath:
    """A filesystem-backed Traversable in the standalone runtime."""

    def __init__(self, path):
        path = os.fspath(path)
        if not isinstance(path, str):
            raise TypeError('resource path must be str or a path-like returning str')
        # Preserve "..": a symlink may precede it during an actual lookup.
        root = '/' if path.startswith('/') else ''
        if path.startswith('//') and not path.startswith('///'):
            root = '//'
        parts = [part for part in path.split('/') if part and part != '.']
        self._path = (root + '/'.join(parts)) or '.'

    def __fspath__(self):
        return self._path

    def __str__(self):
        return self._path

    @property
    def name(self):
        return '' if self._path == '.' else os.path.basename(self._path)

    @property
    def parent(self):
        return _FilePath(os.path.dirname(self._path))

    def joinpath(self, *descendants):
        return _FilePath(os.path.join(self._path, *descendants))

    def __truediv__(self, child):
        return self.joinpath(child)

    def is_file(self):
        return os.path.isfile(self._path)

    def is_dir(self):
        return os.path.isdir(self._path)

    def iterdir(self):
        return (self.joinpath(name) for name in os.listdir(self._path))

    def open(self, mode='r', buffering=-1, encoding=None, errors=None, newline=None):
        return open(self._path, mode, buffering, encoding, errors, newline)

    def read_bytes(self):
        with self.open('rb') as stream:
            return stream.read()

    def read_text(self, encoding=None, errors=None, newline=None):
        with self.open(encoding=encoding, errors=errors, newline=newline) as stream:
            return stream.read()


class FileReader:
    def __init__(self, loader):
        self.path = _FilePath(loader.path).parent

    def files(self):
        return self.path

    def resource_path(self, resource):
        return str(self.path.joinpath(resource))

    def open_resource(self, resource):
        return self.path.joinpath(resource).open('rb')

    def is_resource(self, resource):
        return self.path.joinpath(resource).is_file()

    def contents(self):
        return (item.name for item in self.path.iterdir())
