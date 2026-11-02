import abc as declarations
from abc import ABCMeta as Meta
from dataclasses import dataclass, field


class Reader(metaclass=declarations.ABCMeta):
    @declarations.abstractmethod
    def read(self) -> int:
        pass


class Incomplete(Reader):
    pass


class Concrete(Incomplete):
    def read(self) -> int:
        return 42


class Empty(metaclass=Meta):
    pass


@dataclass
class Default(Reader):
    read: int = field(default=7)


@dataclass
class Factory(Reader):
    read: list[int] = field(default_factory=list)


@dataclass
class Size(declarations.ABC):
    @declarations.abstractmethod
    def __len__(self) -> int:
        pass


class Writer(Reader):
    @declarations.abstractmethod
    def write(self) -> int:
        pass


def local_override() -> None:
    class Partial(Writer):
        read = 42

    try:
        Partial()
    except TypeError as error:
        print(str(error))
    else:
        raise AssertionError("constructed a subclass without write")


try:
    Reader()
except TypeError as error:
    print(str(error))
else:
    raise AssertionError("constructed an abstract class")

try:
    Incomplete()
except TypeError as error:
    print(str(error))
else:
    raise AssertionError("constructed a subclass with an inherited abstract method")

assert Concrete().read() == 42
assert isinstance(Empty(), Empty)
assert Default().read == 7
try:
    Factory()
except TypeError as error:
    print(str(error))
else:
    raise AssertionError("an instance default factory replaced an abstract method")
local_override()
try:
    Size()
except TypeError as error:
    print(str(error))
else:
    raise AssertionError("dataclass replaced an abstract __len__")
