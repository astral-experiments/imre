# The accelerated stat functions take one positional integer-index operand.
from operator import index as _index


class _ModeFunction:
    def __init__(self, function):
        self._function = function
        self.__name__ = self.__qualname__ = function.__name__
        self.__module__ = "_stat"

    def __call__(self, *args, **kwargs):
        name = self.__name__
        if kwargs:
            raise TypeError(f"_stat.{name}() takes no keyword arguments")
        if len(args) != 1:
            raise TypeError(f"_stat.{name}() takes exactly one argument ({len(args)} given)")
        return self._function(_index(args[0]))


for _name, _value in list(globals().items()):
    if (_name.startswith("S_") or _name == "filemode") and callable(_value):
        globals()[_name] = _ModeFunction(_value)
