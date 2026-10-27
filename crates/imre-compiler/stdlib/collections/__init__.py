"""Dictionary subclasses for the runtime collections module."""

import operator as _operator
from _collections import _count_elements, _ordered_dict_view
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
        name = type(self).__name__
        return name + "(" + (dict.__repr__(self) if self else "") + ")"

    def __eq__(self, other):
        if not isinstance(other, dict):
            return NotImplemented
        if isinstance(other, OrderedDict):
            return list(dict.items(self)) == list(dict.items(other))
        return dict(self) == dict(other)

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


class Counter(dict):
    def __init__(self, iterable=None, /, **counts):
        super().__init__()
        self.update(iterable, **counts)

    def __missing__(self, key):
        return 0

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
