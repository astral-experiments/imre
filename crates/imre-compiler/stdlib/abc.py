"""Abstract method declarations for native Python classes."""


def abstractmethod(funcobj):
    funcobj.__isabstractmethod__ = True
    return funcobj


class ABC:
    __slots__ = ()
