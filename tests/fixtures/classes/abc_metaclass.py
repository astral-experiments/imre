import abc as declarations
from abc import ABCMeta as Meta


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
