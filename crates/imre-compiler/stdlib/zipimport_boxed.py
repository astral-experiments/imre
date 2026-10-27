from _imre_zipimport import exists as _exists, index as _index, read as _read
from _imre_posixpath import isfile as _is_file


class ZipImportError(ImportError):
    msg = name = path = name_from = None

    def __init__(self, *args, name=None, path=None, name_from=None):
        super().__init__(*args)
        self.msg = args[0] if len(args) == 1 else None
        self.name = name
        self.path = path
        self.name_from = name_from
