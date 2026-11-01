"""Load Python modules from registered code or filesystem source."""

from _imre_importlib import get_code as _registered_code, loader_fields as _loader_fields
from os import sep


def _get_cached(origin):
    # The standalone runtime has no bytecode cache tag.
    return origin if origin.endswith(".pyc") else None


class SourceFileLoader:
    def __init__(self, fullname, path):
        self.name = fullname
        self.path = path

    def __eq__(self, other):
        return self.__class__ == other.__class__ and _loader_fields(self) == _loader_fields(other)

    def __hash__(self):
        return hash(self.name) ^ hash(self.path)

    def get_filename(self, fullname=None):
        if fullname is not None and fullname != self.name:
            raise ImportError(f"loader for {self.name} cannot handle {fullname}", name=fullname)
        return self.path

    def is_package(self, fullname):
        filename = self.get_filename(fullname).rpartition(sep)[2]
        return filename.rsplit(".", 1)[0] == "__init__" and fullname.rpartition(".")[2] != "__init__"

    def get_data(self, path):
        with open(path, "rb") as file:
            return file.read()

    def get_code(self, fullname):
        path = self.get_filename(fullname)
        code = _registered_code(self.name, path)
        if code is not None:
            return code
        return compile(self.get_data(path), path, "exec")

    def exec_module(self, module):
        exec(self.get_code(module.__name__), module.__dict__)
