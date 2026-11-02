# CPython 3.14.5 Lib/contextlib.py
import abc
import _collections_abc
from types import GenericAlias


class AbstractAsyncContextManager(abc.ABC):

    """An abstract base class for asynchronous context managers."""

    __class_getitem__ = classmethod(GenericAlias)

    __slots__ = ()

    async def __aenter__(self):
        """Return `self` upon entering the runtime context."""
        return self

    @abc.abstractmethod
    async def __aexit__(self, exc_type, exc_value, traceback):
        """Raise any exception triggered within the runtime context."""
        return None

    @classmethod
    def __subclasshook__(cls, C):
        if cls is AbstractAsyncContextManager:
            return _collections_abc._check_methods(C, "__aenter__",
                                                   "__aexit__")
        return NotImplemented
