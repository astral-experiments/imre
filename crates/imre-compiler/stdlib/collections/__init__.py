"""Dictionary and mapping classes for the runtime collections module."""

import operator as _operator
from _collections import _count_elements, _ordered_dict_equal, _ordered_dict_repr, _ordered_dict_view
from _collections_abc import MutableMapping as _MutableMapping
from itertools import repeat as _repeat


def _elements(items):
    for key, count in items:
        yield from _repeat(key, count)


def _compare_counts(left, right, compare):
    if not isinstance(right, Counter):
        return NotImplemented
    for counter in (left, right):
        for key in counter:
            if not compare(left[key], right[key]):
                return False
    return True


class OrderedDict(dict):
    def __iter__(self):
        return iter(_ordered_dict_view(self, "keys"))

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

    def copy(self):
        return type(self)(self)

    @classmethod
    def fromkeys(cls, iterable, value=None):
        result = cls()
        for key in iterable:
            result[key] = value
        return result


_chainmap_repr_stack = []


class ChainMap(_MutableMapping):
    def __init__(self, *maps):
        self.maps = list(maps) or [{}]

    def __getitem__(self, key):
        for mapping in self.maps:
            try:
                return mapping[key]
            except KeyError:
                pass
        return self.__missing__(key)

    def __missing__(self, key):
        raise KeyError(key)

    def __contains__(self, key):
        return any(key in mapping for mapping in self.maps)

    def get(self, key, default=None):
        return self[key] if key in self else default

    def __setitem__(self, key, value):
        self.maps[0][key] = value

    def __delitem__(self, key):
        try:
            del self.maps[0][key]
        except KeyError:
            raise KeyError(f"Key not found in the first mapping: {key!r}")

    def __iter__(self):
        keys = {}
        for mapping in reversed(self.maps):
            keys.update(dict.fromkeys(mapping))
        return iter(keys)

    def __len__(self):
        return len(set(iter(self)))

    def __bool__(self):
        return any(self.maps)

    def __repr__(self):
        for owner in _chainmap_repr_stack:
            if owner is self:
                return "..."
        _chainmap_repr_stack.append(self)
        try:
            return f"{type(self).__name__}({', '.join(map(repr, self.maps))})"
        finally:
            _chainmap_repr_stack.pop()


class Counter(dict):
    def __init__(self, iterable=None, /, **counts):
        super().__init__()
        self.update(iterable, **counts)

    def __missing__(self, key):
        return 0

    @classmethod
    def fromkeys(cls, iterable, v=None):
        raise NotImplementedError("Counter.fromkeys() is undefined.  Use Counter(iterable) instead.")

    def update(self, iterable=None, /, **counts):
        if isinstance(iterable, dict):
            if self:
                get = self.get
                for key, count in iterable.items():
                    self[key] = count + get(key, 0)
            else:
                super().update(iterable)
        elif iterable is not None:
            _count_elements(self, iterable)
        if counts:
            self.update(counts)

    def subtract(self, iterable=None, /, **counts):
        if iterable is not None:
            get = self.get
            if isinstance(iterable, dict):
                for key, count in iterable.items():
                    self[key] = get(key, 0) - count
            else:
                for key in iterable:
                    self[key] = get(key, 0) - 1
        if counts:
            self.subtract(counts)

    def most_common(self, n=None):
        items = self.items()
        if n is not None and n <= 0:
            return []
        ranked = sorted(items, key=_operator.itemgetter(1), reverse=True)
        return ranked if n is None or n >= len(ranked) else ranked[:n]

    def __repr__(self):
        name = type(self).__name__
        if not self:
            return f"{name}()"
        try:
            entries = dict(self.most_common())
        except TypeError:
            entries = dict(self)
        return f"{name}({entries!r})"

    def total(self):
        return sum(self.values())

    def elements(self):
        return _elements(iter(self.items()))

    def copy(self):
        return self.__class__(self)

    def __delitem__(self, key):
        dict.pop(self, key, None)

    def __eq__(self, other):
        return _compare_counts(self, other, _operator.eq)

    def __ne__(self, other):
        if not isinstance(other, Counter):
            return NotImplemented
        return not self == other

    def __le__(self, other):
        return _compare_counts(self, other, _operator.le)

    def __lt__(self, other):
        if not isinstance(other, Counter):
            return NotImplemented
        return self <= other and self != other

    def __ge__(self, other):
        return _compare_counts(self, other, _operator.ge)

    def __gt__(self, other):
        if not isinstance(other, Counter):
            return NotImplemented
        return self >= other and self != other
