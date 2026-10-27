"""Stream annotations and special forms for the runtime typing module."""

import typing as _typing
from functools import lru_cache
from typing import Generic, TypeVar


AnyStr = TypeVar("AnyStr", bytes, str)


class IO(Generic[AnyStr]):
    __slots__ = ()


class BinaryIO(IO[bytes]):
    __slots__ = ()


class TextIO(IO[str]):
    __slots__ = ()


# Optional and the special form protocol from CPython 3.14.5 Lib/typing.py.
# See stdlib/docstrings/LICENSE. Union and the type parameters are native exports.
class _Final:
    __module__ = __name__
    __slots__ = ("__weakref__",)


class _NotIterable:
    __module__ = __name__
    __slots__ = ()
    __iter__ = None


class _SpecialForm(_Final, _NotIterable):
    __module__ = __name__
    __slots__ = ("_name", "__doc__", "_getitem")

    def __init__(self, getitem):
        self._getitem = getitem
        self._name = getitem.__name__
        self.__doc__ = getitem.__doc__

    def __getattr__(self, name):
        if name in ("__name__", "__qualname__"):
            return self._name
        raise AttributeError(name)

    def __mro_entries__(self, bases):
        raise TypeError(f"Cannot subclass {repr(self)}")

    def __repr__(self):
        return "typing." + self._name

    def __reduce__(self):
        return self._name

    def __call__(self, *args, **kwds):
        raise TypeError(f"Cannot instantiate {repr(self)}")

    def __or__(self, other):
        return _typing.Union[self, other]

    def __ror__(self, other):
        return _typing.Union[other, self]

    def __instancecheck__(self, obj):
        raise TypeError(f"{repr(self)} cannot be used with isinstance()")

    def __subclasscheck__(self, cls):
        raise TypeError(f"{repr(self)} cannot be used with issubclass()")

    @lru_cache
    def _cached_getitem(self, parameters):
        return self._getitem(self, parameters)

    def __getitem__(self, parameters):
        try:
            return self._cached_getitem(parameters)
        except TypeError:
            # Like _tp_cache, retry uncached for unhashable arguments.
            return self._getitem(self, parameters)


def _type_check(arg, message):
    protocol = getattr(_typing, "Protocol", None)
    if isinstance(arg, _SpecialForm) or arg is _typing.Generic or (protocol is not None and arg is protocol):
        raise TypeError(f"Plain {repr(arg)} is not valid as type argument")
    origin = _typing.get_origin(arg)
    if origin is _typing.Generic or (protocol is not None and origin is protocol):
        raise TypeError(f"{repr(arg)} is not valid as type argument")
    if type(arg) is tuple:
        raise TypeError(f"{message} Got {repr(arg)[:100]}.")
    return arg


@_SpecialForm
def Optional(self, parameters):
    """Optional[X] is equivalent to Union[X, None]."""
    arg = _type_check(parameters, f"{repr(self)} requires a single type.")
    return _typing.Union[arg, type(None)]


class _TypedCacheSpecialForm(_SpecialForm):
    __module__ = __name__

    @lru_cache
    def _typed_getitem(self, parameters, types):
        # As in _tp_cache(typed=True), only the immediate argument types are keys.
        return self._getitem(self, *parameters)

    def __getitem__(self, parameters):
        if not isinstance(parameters, tuple):
            parameters = (parameters,)
        try:
            return self._typed_getitem(parameters, tuple(type(p) for p in parameters))
        except TypeError:
            return self._getitem(self, *parameters)


@_TypedCacheSpecialForm
def Literal(self, *parameters):
    """A type whose arguments are literal values."""
    return _typing._LiteralGenericAlias(self, parameters)
