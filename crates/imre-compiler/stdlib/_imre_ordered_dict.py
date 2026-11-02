"""OrderedDict backed by the runtime's ordered dictionary storage."""

from _collections import (
    _ordered_dict_clear,
    _ordered_dict_delitem,
    _ordered_dict_equal,
    _ordered_dict_setitem,
    _ordered_dict_move_to_end,
    _ordered_dict_pop,
    _ordered_dict_popitem,
    _ordered_dict_repr,
    _ordered_dict_setdefault,
    _ordered_dict_view,
)


_ordered_dict_missing = object()


def _ordered_dict_receiver(receiver, method):
    if not issubclass(type(receiver), OrderedDict):
        raise TypeError(f"descriptor '{method}' requires a 'collections.OrderedDict' object "
                        f"but received a '{type(receiver).__name__}'")


def _ordered_dict_update(target, other):
    if type(other) is dict:
        pairs = list(other.items())
    else:
        keys = getattr(other, "keys", _ordered_dict_missing)
        if keys is not _ordered_dict_missing:
            for key in keys():
                target[key] = other[key]
            return
        items = getattr(other, "items", _ordered_dict_missing)
        pairs = other if items is _ordered_dict_missing else items()
    for key, value in pairs:
        target[key] = value


def _ordered_dict_update_args(target, args, kwargs):
    if len(args) > 1:
        raise TypeError(f"expected at most 1 argument, got {len(args)}")
    if args:
        _ordered_dict_update(target, args[0])
    if kwargs:
        _ordered_dict_update(target, kwargs)


def _ordered_dict_union(left, right):
    if not issubclass(type(left), dict) or not issubclass(type(right), dict):
        return NotImplemented
    cls = type(left) if issubclass(type(left), OrderedDict) else type(right)
    result = cls(left)
    _ordered_dict_update(result, right)
    return result


class OrderedDict(dict):
    __module__ = "collections"

    def __init__(self, /, *args, **kwargs):
        _ordered_dict_receiver(self, "__init__")
        _ordered_dict_update_args(self, args, kwargs)

    def update(self, /, *args, **kwargs):
        _ordered_dict_receiver(self, "update")
        _ordered_dict_update_args(self, args, kwargs)

    def __setitem__(self, key, value, /):
        _ordered_dict_receiver(self, "__setitem__")
        _ordered_dict_setitem(self, key, value)

    def __delitem__(self, key, /):
        _ordered_dict_receiver(self, "__delitem__")
        _ordered_dict_delitem(self, key)

    def clear(self, /):
        _ordered_dict_receiver(self, "clear")
        _ordered_dict_clear(self)

    def __iter__(self):
        return iter(_ordered_dict_view(self, "keys"))

    def __reversed__(self):
        return reversed(_ordered_dict_view(self, "keys"))

    def keys(self):
        return _ordered_dict_view(self, "keys")

    def items(self):
        return _ordered_dict_view(self, "items")

    def values(self):
        return _ordered_dict_view(self, "values")

    def __repr__(self):
        return _ordered_dict_repr(self)

    def __eq__(self, other):
        return _ordered_dict_equal(self, other, isinstance(other, OrderedDict))

    def __ne__(self, other):
        equal = self.__eq__(other)
        return NotImplemented if equal is NotImplemented else not equal

    def __or__(self, other, /):
        _ordered_dict_receiver(self, "__or__")
        return _ordered_dict_union(self, other)

    def __ror__(self, other, /):
        _ordered_dict_receiver(self, "__ror__")
        return _ordered_dict_union(other, self)

    def __ior__(self, other, /):
        _ordered_dict_receiver(self, "__ior__")
        _ordered_dict_update(self, other)
        return self

    def move_to_end(self, /, key, last=True):
        return _ordered_dict_move_to_end(self, key, last)

    def pop(self, /, key, default=_ordered_dict_missing):
        if default is _ordered_dict_missing:
            return _ordered_dict_pop(self, key)
        return _ordered_dict_pop(self, key, default)

    def setdefault(self, /, key, default=None):
        if type(self) is OrderedDict:
            return _ordered_dict_setdefault(self, key, default)
        if key in self:
            return self[key]
        self[key] = default
        return default

    def popitem(self, /, last=True):
        return _ordered_dict_popitem(self, last)

    def __reduce__(self, /):
        return type(self), (), self.__getstate__(), None, iter(self.items())

    def copy(self):
        return type(self)(self)

    @classmethod
    def fromkeys(cls, iterable, value=None):
        result = cls()
        for key in iterable:
            result[key] = value
        return result
