import builtins
import sys

from types import GenericAlias


class Unpack:
    def __getitem__(self, pack):
        print("unpack", pack.__name__)
        return pack

class Typing:
    Unpack = Unpack()
    @staticmethod
    def _GenericAlias(origin, params):
        received.append(params)
        return GenericAlias(origin, params)
    @staticmethod
    def _generic_init_subclass(cls):
        pass

def observe_import(name, *args, **kwargs):
    if name == "typing":
        print("import typing")
    return original_import(name, *args, **kwargs)

received = []
sys.modules["typing"] = Typing()
original_import = builtins.__import__
builtins.__import__ = observe_import

class Box[T]: pass
print("single", received[-1] is Box.__type_params__)

class Callback[**P]: pass
print("paramspec", received[-1] is Callback.__type_params__)

class Group[*Ts]: pass
print("pack", received[-1] is Group.__type_params__)
