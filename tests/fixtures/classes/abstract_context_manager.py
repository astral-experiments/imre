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


def use(resource: AbstractContextManager[int]) -> int:
    return resource.__enter__()


def incomplete() -> None:
    class MissingExit(AbstractContextManager):
        pass

    MissingExit()


assert use(Resource()) == 42
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
