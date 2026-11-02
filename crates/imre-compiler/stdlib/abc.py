"""Abstract method declarations for native Python classes."""

from abc import ABCMeta  # Supplied by the native module initializer.


def abstractmethod(funcobj):
    funcobj.__isabstractmethod__ = True
    return funcobj


class ABC(metaclass=ABCMeta):
    __slots__ = ()
