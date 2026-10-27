import typing
from abc import abstractmethod
from typing import Protocol


class HasHTML(Protocol):
    def __html__(self):
        ...


class Concrete(HasHTML):
    def __html__(self):
        return "trusted"


class WithInit(Protocol):
    def __init__(self, value):
        self.value = value


class Abstract(Protocol):
    @abstractmethod
    def operation(self):
        ...


def current():
    print(typing.TYPE_CHECKING is False, typing.Protocol is Protocol)
    print(Protocol.__bases__ == (typing.Generic,), HasHTML.__mro__[1:3] == (Protocol, typing.Generic))
    print(HasHTML._is_protocol, Concrete._is_protocol, Concrete().__html__())
    print(WithInit("explicit initializer").value)
    print(Protocol.__abstractmethods__ == frozenset(), Abstract.__abstractmethods__ == {"operation"})
    parameter = typing.TypeVar("T")
    alias = Protocol[parameter]
    print(alias.__origin__ is Protocol, alias.__args__[0] is parameter)
    for check in (
        lambda: HasHTML(),
        lambda: Abstract(),
        lambda: isinstance("text", (HasHTML, str)),
        lambda: issubclass(Concrete, HasHTML),
    ):
        try:
            check()
        except TypeError as error:
            print(str(error))
    print(isinstance("text", (str, HasHTML)), isinstance(Concrete(), Concrete))
