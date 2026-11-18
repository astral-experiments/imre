"""Source package imports for the native module registry."""
from _imre_importlib import _zip_path_importers as _path_importers, compile_file as _compile_file


class zipimporter(_ZipResourceLoader):
    def __init__(self, path):
        linked = _linked_archive(path)
        self._has_linked = linked is not None
        self._linked_enabled = self._has_linked
        if linked is None:
            super().__init__(path)
        else:
            self.archive, self.prefix = linked
            self._indexed_archive = self.archive

    def invalidate_caches(self):
        """Invalidate the cached directory of this importer's archive."""
        _invalidate(self.archive)
        self._linked_enabled = self._has_linked

    def _member(self, fullname):
        if self._linked_enabled:
            origin = _linked_member(self.archive + '/' + self.prefix, fullname)
            return None if origin is None else origin[len(self.archive) + 1:]
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

    def get_resource_reader(self, fullname):
        """Return the resource reader for a module in this ZIP archive."""
        from importlib.readers import ZipReader

        return ZipReader(self, fullname)

    def get_source(self, fullname):
        return self.get_data(self._source_member(fullname)).decode('utf-8')

    def get_code(self, fullname):
        filename = self.get_filename(fullname)
        if self._linked_enabled:
            code = _linked_code(fullname, filename)
            if code is None:
                raise ZipImportError(f"no linked code for {fullname!r}", name=fullname)
            return code
        return _compile_file(compile, self.get_data(filename), filename)

    def create_module(self, spec):
        # Use the standard module object allocated by module_from_spec.
        return None

    def exec_module(self, module):
        from _frozen_importlib_external import _LoaderBasics
        return _LoaderBasics.exec_module(self, module)

    def _origin(self, fullname):
        entry = self._member(fullname)
        if entry is not None:
            return self.archive + '/' + entry, self
        directory = self.prefix + fullname.rpartition('.')[2] + '/'
        if self._linked_enabled:
            namespace = _linked_namespace(self.archive + '/' + self.prefix, fullname)
            return None if namespace is None else (namespace, None)
        if _contains(self._indexed_archive, directory):
            return self.archive + '/' + directory[:-1], None
        return None

    def find_spec(self, fullname, target=None):
        resolved = self._origin(fullname)
        if resolved is None:
            return None
        from _frozen_importlib import ModuleSpec
        origin, loader = resolved
        if loader is None:
            spec = ModuleSpec(fullname, None, is_package=True)
            spec.submodule_search_locations.append(origin)
            return spec
        locations = [origin.rpartition('/')[0]] if origin.endswith('/__init__.py') else None
        spec = ModuleSpec(fullname, loader, origin=origin)
        spec.submodule_search_locations = locations
        spec.has_location = True
        return spec


def _find_at(root, fullname):
    if root not in _path_importers:
        try:
            _path_importers[root] = zipimporter(root)
        except ZipImportError:
            _path_importers[root] = None
    finder = _path_importers[root]
    return None if finder is None else finder._origin(fullname)
