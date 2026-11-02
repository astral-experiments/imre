import sys as _sys


def namedtuple(typename, field_names, *, rename=False, defaults=None, module=None):
    result = _namedtuple_factory(typename, field_names, rename=rename, defaults=defaults, module=module)
    if module is None:
        try:
            module = _sys._getframemodulename(1) or "__main__"
        except AttributeError:
            try:
                module = _sys._getframe(1).f_globals.get("__name__", "__main__")
            except (AttributeError, ValueError):
                pass
        if module is not None:
            result.__module__ = module
    return result


def _namedtuple_repr(typename, field_names):
    repr_fmt = '(' + ', '.join(f'{name}=%r' for name in field_names) + ')'

    def __repr__(self):
        'Return a nicely formatted representation string'
        return self.__class__.__name__ + repr_fmt % self

    __repr__.__qualname__ = f'{typename}.__repr__'
    return __repr__


def _namedtuple_make(typename, field_names):
    tuple_new, _len = tuple.__new__, len
    num_fields = _len(field_names)

    @classmethod
    def _make(cls, iterable):
        result = tuple_new(cls, iterable)
        if _len(result) != num_fields:
            raise TypeError(f'Expected {num_fields} arguments, got {len(result)}')
        return result

    _make.__func__.__doc__ = f'Make a new {typename} object from a sequence or iterable'
    _make.__func__.__qualname__ = f'{typename}._make'
    return _make


def _namedtuple_replace(typename, field_names):
    _map = map

    def _replace(self, /, **kwds):
        result = self._make(_map(kwds.pop, field_names, self))
        if kwds:
            raise TypeError(f'Got unexpected field names: {list(kwds)!r}')
        return result

    _replace.__doc__ = f'Return a new {typename} object replacing specified fields with new values'
    _replace.__qualname__ = f'{typename}._replace'
    return _replace


def _namedtuple_getnewargs(typename, with_asdict=False):
    def __getnewargs__(self):
        'Return self as a plain tuple. Used by copy and pickle.'
        return tuple(self)

    __getnewargs__.__qualname__ = f'{typename}.__getnewargs__'
    if not with_asdict:
        return __getnewargs__

    _dict, _zip = dict, zip

    def _asdict(self):
        'Return a new dict which maps field names to their values.'
        return _dict(_zip(self._fields, self))

    _asdict.__qualname__ = f'{typename}._asdict'
    return __getnewargs__, _asdict
