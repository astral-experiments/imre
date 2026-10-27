"""Iterator tools for runtime-loaded Python."""


class chain:
    """Yield items from each iterable in order."""

    __slots__ = ("_sources", "_active")

    def __init__(self, *iterables):
        self._sources = iter(iterables)
        self._active = None

    def __iter__(self):
        return self

    def __next__(self):
        while self._sources is not None:
            if self._active is None:
                try:
                    self._active = iter(next(self._sources))
                except BaseException:
                    self._sources = None
                    raise
            try:
                return next(self._active)
            except StopIteration:
                self._active = None
        raise StopIteration


_missing = object()


class groupby:
    """Group consecutive values with equal keys, sharing one input iterator."""

    __slots__ = ("_source", "_key", "_item", "_target", "_generation")

    def __init__(self, iterable, key=None):
        self._source = iter(iterable)
        self._key = key
        self._item = None
        self._target = _missing
        self._generation = 0

    def __iter__(self):
        return self

    def _advance(self):
        value = next(self._source)
        key = value if self._key is None else self._key(value)
        self._item = key, value

    def __next__(self):
        self._generation += 1
        if self._item is None:
            self._advance()
        while self._target is not _missing:
            key = self._item[0]
            if self._target is not key and not (self._target == key):
                break
            self._advance()
        self._target = self._item[0]
        return self._target, _grouper(self, self._target, self._generation)


class _grouper:
    __slots__ = ("_parent", "_target", "_generation")

    def __init__(self, parent, target, generation):
        self._parent = parent
        self._target = target
        self._generation = generation

    def __iter__(self):
        return self

    def __next__(self):
        parent = self._parent
        if self._generation != parent._generation:
            raise StopIteration
        if parent._item is None:
            parent._advance()
        key = parent._item[0]
        if self._target is not key and not (self._target == key):
            raise StopIteration
        if parent._item is None:
            raise StopIteration
        value = parent._item[1]
        parent._item = None
        return value
