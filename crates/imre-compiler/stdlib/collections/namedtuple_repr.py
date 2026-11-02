_namedtuple_factory = namedtuple


def namedtuple(typename, field_names, *, rename=False, defaults=None, module=None):
    result = _namedtuple_factory(typename, field_names, rename=rename, defaults=defaults, module=module)
    if module is None:
        import sys
        try:
            module = sys._getframemodulename(1) or "__main__"
        except AttributeError:
            try:
                module = sys._getframe(1).f_globals.get("__name__", "__main__")
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
