"""OrderedDict backed by the runtime's ordered dictionary storage."""

from _collections import (
    _ordered_dict_equal,
    _ordered_dict_move_to_end,
    _ordered_dict_pop,
    _ordered_dict_popitem,
    _ordered_dict_repr,
    _ordered_dict_setdefault,
    _ordered_dict_view,
)


_ordered_dict_missing = object()


class OrderedDict(dict):
    __module__ = "collections"

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
