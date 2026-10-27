"""Integer counters for the runtime collections module."""

import operator as _operator


def _count_elements(mapping, iterable):
    get = mapping.get
    for key in iterable:
        mapping[key] = get(key, 0) + 1


def _compare_counts(left, right, compare):
    if not isinstance(right, Counter):
        return NotImplemented
    for counter in (left, right):
        for key in counter:
            if not compare(left[key], right[key]):
                return False
    return True


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
