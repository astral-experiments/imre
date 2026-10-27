"""Source package imports for the native module registry."""
from _imre_zipimport import contains as _contains

_ENCODING_NAME_CHARS = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_."


class zipimporter(_ZipResourceLoader):
    def _member(self, fullname):
        stem = self.prefix + fullname.rpartition('.')[2]
        for suffix in ('/__init__.pyc', '/__init__.py', '.pyc', '.py'):
            entry = stem + suffix
            if _contains(self._indexed_archive, entry):
                if suffix.endswith('.pyc'):
                    raise NotImplementedError('ZIP bytecode imports are unsupported')
                return entry
        return None

    def _source_member(self, fullname):
        entry = self._member(fullname)
        if entry is None:
            raise ZipImportError(f"can't find module {fullname!r}", name=fullname)
        return entry

    def get_filename(self, fullname):
        return self.archive + '/' + self._source_member(fullname)

    def is_package(self, fullname):
        return self._source_member(fullname).endswith('/__init__.py')

    def get_source(self, fullname):
        return self.get_data(self._source_member(fullname)).decode('utf-8')

    def get_code(self, fullname):
        filename = self.get_filename(fullname)
        text = self.get_data(filename).decode('utf-8').removeprefix('\ufeff')
        text = text.replace('\r\n', '\n').replace('\r', '\n')
        for line in text.split('\n', 2)[:2]:
            line = line.lstrip(' \t\f')
            if line and not line.startswith('#'):
                break
            for marker in ('coding:', 'coding='):
                offset = line.find(marker)
                while offset >= 0:
                    offset += len(marker)
                    while offset < len(line) and line[offset] in ' \t':
                        offset += 1
                    if offset < len(line) and line[offset] in _ENCODING_NAME_CHARS:
                        raise NotImplementedError('encoding declarations in ZIP source are unsupported')
                    offset = line.find(marker, offset)
        return compile(text, filename, 'exec')

    def exec_module(self, module):
        exec(self.get_code(module.__name__), module.__dict__)

    def _origin(self, fullname):
        entry = self._member(fullname)
        if entry is not None:
            return self.archive + '/' + entry, self
        directory = self.prefix + fullname.rpartition('.')[2] + '/'
        if _contains(self._indexed_archive, directory):
            return self.archive + '/' + directory[:-1], None
        return None

    def find_spec(self, fullname, target=None):
        from importlib import _file_spec
        resolved = self._origin(fullname)
        if resolved is None:
            return None
        origin, loader = resolved
        if loader is None:
            raise NotImplementedError('ZIP namespace packages are unsupported')
        locations = [origin.rpartition('/')[0]] if origin.endswith('/__init__.py') else None
        return _file_spec(fullname, origin, locations, loader)


_path_importers = {}


def _find_at(root, fullname):
    if root not in _path_importers:
        try:
            _path_importers[root] = zipimporter(root)
        except ZipImportError:
            return None
    return _path_importers[root]._origin(fullname)
