def _namedtuple_repr(typename, field_names):
    repr_fmt = '(' + ', '.join(f'{name}=%r' for name in field_names) + ')'

    def __repr__(self):
        'Return a nicely formatted representation string'
        return self.__class__.__name__ + repr_fmt % self

    __repr__.__qualname__ = f'{typename}.__repr__'
    return __repr__
