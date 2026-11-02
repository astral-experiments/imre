"""The loader protocol shared by importlib.abc and importlib.util."""

import abc
from . import _bootstrap


class Loader(metaclass=abc.ABCMeta):
    def create_module(self, spec):
        return None

    def load_module(self, fullname):
        if not hasattr(self, "exec_module"):
            raise ImportError
        return _bootstrap._load_module_shim(self, fullname)
