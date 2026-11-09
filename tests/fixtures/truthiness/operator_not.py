from abc import ABC, abstractmethod
from binascii import b2a_base64
from operator import __not__ as negate
from operator import not_


class Default(ABC):
    pass


class AbstractFlag(Default):
    @abstractmethod
    def other(self) -> bool:
        pass

    def __bool__(self) -> bool:
        return False


class Flag:
    pass


class Empty(Flag):
    def __bool__(self) -> bool:
        print("truth")
        return False


def make() -> Flag:
    print("make")
    return Empty()


def test(value: Flag | None) -> None:
    print(negate(value))
    print(b2a_base64(b"a", newline=value))
    print(not_(make() and value))


def classify(value: Flag | Exception) -> None:
    print(isinstance(value, Exception))
    print(not_(value))


print(not_(Default()))
test(Flag())
test(Empty())
test(None)
classify(make())
classify(ValueError("other"))
