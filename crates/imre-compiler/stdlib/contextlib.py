from abc import ABC, abstractmethod


class AbstractContextManager(ABC):
    __slots__ = ()

    def __enter__(self) -> "AbstractContextManager":
        return self

    @abstractmethod
    def __exit__(self, exc_type: object, exc_value: object, traceback: object) -> None:
        pass
