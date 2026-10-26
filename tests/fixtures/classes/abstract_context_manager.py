import contextlib
from contextlib import AbstractContextManager


class Resource(AbstractContextManager[int]):
    def __enter__(self) -> int:
        return 42

    def __exit__(self, exc_type: object, value: object, tb: object) -> None:
        return super().__exit__(exc_type, value, tb)


class DefaultEnter(contextlib.AbstractContextManager):
    __slots__ = ()

    def __exit__(self, exc_type: object, value: object, tb: object) -> None:
        return None


class Duck:
    def __enter__(self) -> "Duck":
        return self

    def __exit__(self, exc_type: object, value: object, tb: object) -> None:
        return None


class Missing(Duck):
    __enter__ = None


def is_manager(resource: AbstractContextManager[int]) -> bool:
    return isinstance(resource, AbstractContextManager)


def local_protocol() -> bool:
    class Both:
        def __enter__(self):
            return self

        def __exit__(self, typ, value, tb):
            return None

    class NoExit(Both):
        __exit__ = None

    return issubclass(Both, AbstractContextManager) and not issubclass(NoExit, AbstractContextManager)


def incomplete() -> None:
    class MissingExit(AbstractContextManager):
        pass

    MissingExit()


assert local_protocol()
assert is_manager(Resource())
assert Resource().__enter__() == 42
assert Resource().__exit__(None, None, None) is None
manager = DefaultEnter()
assert manager.__enter__() is manager
assert issubclass(Duck, AbstractContextManager)
assert not issubclass(Missing, AbstractContextManager)
assert isinstance(Duck(), AbstractContextManager)
assert not isinstance(Missing(), AbstractContextManager)
try:
    incomplete()
except TypeError as error:
    print(str(error))
else:
    raise AssertionError("constructed an abstract class")
try:
    manager.attribute = 42
except AttributeError as error:
    print(str(error))
else:
    raise AssertionError("empty slots accepted a new attribute")
