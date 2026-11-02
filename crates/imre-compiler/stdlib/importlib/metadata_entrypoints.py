# Adapted from CPython 3.14.5 Lib/importlib/metadata/__init__.py (PSF-2.0).
class EntryPoint:
    pattern = re.compile(
        r'(?P<module>[\w.]+)\s*'
        r'(:\s*(?P<attr>[\w.]+)\s*)?'
        r'((?P<extras>\[.*\])\s*)?$'
    )
    dist = None

    def __init__(self, name, value, group):
        vars(self).update(name=name, value=value, group=group)

    def __setattr__(self, name, value):
        raise AttributeError("EntryPoint objects are immutable.")

    def _key(self):
        return self.name, self.value, self.group

    def __eq__(self, other):
        return self._key() == other._key()

    def __lt__(self, other):
        return self._key() < other._key()

    def __hash__(self):
        return hash(self._key())

    def __repr__(self):
        return f'EntryPoint(name={self.name!r}, value={self.value!r}, group={self.group!r})'

    def _field(self, name):
        match = self.pattern.match(self.value)
        assert match is not None
        return match.group(name)

    @property
    def module(self):
        return self._field('module')

    @property
    def attr(self):
        return self._field('attr')

    @property
    def extras(self):
        return re.findall(r'\w+', self._field('extras') or '')

    def load(self):
        from importlib import import_module
        match = self.pattern.match(self.value)
        obj = import_module(match.group('module'))
        for part in filter(None, (match.group('attr') or '').split('.')):
            obj = getattr(obj, part)
        return obj

    def matches(self, **params):
        return all(value == getattr(self, key) for key, value in params.items())


class EntryPoints(tuple):
    __slots__ = ()

    def __getitem__(self, name):
        try:
            return next(iter(self.select(name=name)))
        except StopIteration:
            raise KeyError(name)

    def __repr__(self):
        return f'{type(self).__name__}({tuple(self)!r})'

    def select(self, **params):
        return EntryPoints(ep for ep in self if ep.matches(**params))

    @property
    def names(self):
        return {ep.name for ep in self}

    @property
    def groups(self):
        return {ep.group for ep in self}


def _dist_entry_points(dist):
    group = None
    entries = []
    for line in (dist.read_text('entry_points.txt') or '').splitlines():
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        if line.startswith('[') and line.endswith(']'):
            group = line.strip('[]')
        elif group is not None:
            name, value = line.split('=', 1)
            entry = EntryPoint(name.strip(), value.strip(), group)
            vars(entry).update(dist=dist)
            entries.append(entry)
    return EntryPoints(entries)


def entry_points(**params):
    entries = (ep for dist in _unique(distributions()) for ep in dist.entry_points)
    return EntryPoints(entries).select(**params)
