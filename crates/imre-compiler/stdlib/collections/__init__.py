"""Dictionary and mapping classes for the runtime collections module."""

import _collections_abc
import operator as _operator
from _collections_abc import MutableMapping as _MutableMapping, MutableSequence as _MutableSequence
from itertools import repeat as _repeat

try:
    from _collections import _count_elements
except ImportError:
    def _count_elements(mapping, iterable):
        mapping_get = mapping.get
        for elem in iterable:
            mapping[elem] = mapping_get(elem, 0) + 1

# Pickle resolves the forward iterator's factory in this module.
try:
    from _collections import _deque_iterator
except ImportError:
    pass

from _collections import deque
_MutableSequence.register(deque)


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


def _combine_counts(left, right, op):
    if not isinstance(right, Counter):
        return NotImplemented
    result = Counter()
    for key, count in left.items():
        other = right[key]
        if op == "+":
            value = count + other
        elif op == "-":
            value = count - other
        elif op == "|":
            value = other if count < other else count
        else:
            value = count if count < other else other
        if value > 0:
            result[key] = value
    if op != "&":
        for key, count in right.items():
            if key not in left:
                value = 0 - count if op == "-" else count
                if value > 0:
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

    # From CPython 3.14.5, Lib/collections/__init__.py (licenses/CPython.txt).
    def copy(self):
        'New ChainMap or subclass with a new copy of maps[0] and refs to maps[1:]'
        return self.__class__(self.maps[0].copy(), *self.maps[1:])

    __copy__ = copy

    def new_child(self, m=None, **kwargs):      # like Django's Context.push()
        '''New ChainMap with a new map followed by all previous maps.
        If no map is provided, an empty dict is used.
        Keyword arguments update the map or new empty dict.
        '''
        if m is None:
            m = kwargs
        elif kwargs:
            m.update(kwargs)
        return self.__class__(m, *self.maps)

    @property
    def parents(self):                          # like Django's Context.pop()
        'New ChainMap from maps[1:].'
        return self.__class__(*self.maps[1:])

    def popitem(self):
        'Remove and return an item pair from maps[0]. Raise KeyError is maps[0] is empty.'
        try:
            return self.maps[0].popitem()
        except KeyError:
            raise KeyError('No keys found in the first mapping.')

    def pop(self, key, *args):
        'Remove *key* from maps[0] and return its value. Raise KeyError if *key* not in maps[0].'
        try:
            return self.maps[0].pop(key, *args)
        except KeyError:
            raise KeyError(f'Key not found in the first mapping: {key!r}')

    def clear(self):
        'Clear maps[0], leaving maps[1:] intact.'
        self.maps[0].clear()

    def __ior__(self, other):
        self.maps[0].update(other)
        return self

    def __or__(self, other):
        if not isinstance(other, _collections_abc.Mapping):
            return NotImplemented
        m = self.copy()
        m.maps[0].update(other)
        return m

    def __ror__(self, other):
        if not isinstance(other, _collections_abc.Mapping):
            return NotImplemented
        m = dict(other)
        for child in reversed(self.maps):
            m.update(child)
        return self.__class__(m)


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
        if iterable is not None:
            if isinstance(iterable, _collections_abc.Mapping):
                if self:
                    get = self.get
                    for key, count in iterable.items():
                        self[key] = count + get(key, 0)
                else:
                    super().update(iterable)
            else:
                _count_elements(self, iterable)
        if counts:
            self.update(counts)

    def subtract(self, iterable=None, /, **counts):
        if iterable is not None:
            get = self.get
            if isinstance(iterable, _collections_abc.Mapping):
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

    def __reduce__(self):
        return self.__class__, (dict(self),)

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

    def __pos__(self):
        return Counter({key: count for key, count in self.items() if count > 0})

    def __neg__(self):
        return Counter({key: 0 - count for key, count in self.items() if count < 0})

    def __add__(self, other):
        return _combine_counts(self, other, "+")

    def __sub__(self, other):
        return _combine_counts(self, other, "-")

    def __or__(self, other):
        return _combine_counts(self, other, "|")

    def __and__(self, other):
        return _combine_counts(self, other, "&")

    def _keep_positive(self):
        for key in [key for key, count in self.items() if not count > 0]:
            del self[key]
        return self

    def __iadd__(self, other):
        for key, count in other.items():
            self[key] += count
        return self._keep_positive()

    def __isub__(self, other):
        for key, count in other.items():
            self[key] -= count
        return self._keep_positive()

    def __ior__(self, other):
        for key, count in other.items():
            if count > self[key]:
                self[key] = count
        return self._keep_positive()

    def __iand__(self, other):
        for key, count in self.items():
            value = other[key]
            if value < count:
                self[key] = value
        return self._keep_positive()
