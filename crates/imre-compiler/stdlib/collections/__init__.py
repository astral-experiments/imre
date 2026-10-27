"""Integer counters for the runtime collections module."""


def _count_elements(mapping, iterable):
    get = mapping.get
    for key in iterable:
        mapping[key] = get(key, 0) + 1


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
