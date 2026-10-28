from abc import ABC, abstractmethod


class AbstractContextManager(ABC):
    __slots__ = ()

    def __enter__(self) -> "AbstractContextManager":
        return self

    @abstractmethod
    def __exit__(self, exc_type: object, exc_value: object, traceback: object) -> None:
        pass


class nullcontext(AbstractContextManager):
    def __init__(self, enter_result=None):
        self.enter_result = enter_result

    def __enter__(self):
        return self.enter_result

    def __exit__(self, *excinfo):
        pass
