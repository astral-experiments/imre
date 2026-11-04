"""Abstract method declarations for native Python classes."""

from abc import ABCMeta  # Supplied by the native module initializer.


def abstractmethod(funcobj):
    funcobj.__isabstractmethod__ = True
    return funcobj


def update_abstractmethods(cls):
    """Recalculate a class's abstract methods after its definition changes."""
    if not hasattr(cls, '__abstractmethods__'):
        return cls

    abstracts = set()
    for base in cls.__bases__:
        for name in getattr(base, '__abstractmethods__', ()):
            if getattr(getattr(cls, name, None), '__isabstractmethod__', False):
                abstracts.add(name)
    for name, value in cls.__dict__.items():
        if getattr(value, '__isabstractmethod__', False):
            abstracts.add(name)
    cls.__abstractmethods__ = frozenset(abstracts)
    return cls


class ABC(metaclass=ABCMeta):
    __slots__ = ()
