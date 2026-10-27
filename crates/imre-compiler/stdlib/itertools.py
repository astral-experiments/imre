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
