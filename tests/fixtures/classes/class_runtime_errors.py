import abc
import copy


class Base:
    def __reduce_ex__(self, protocol: int) -> int:
        print("reduce", protocol)
        return 0


class Child(Base):
    pass


class WithState:
    def __getstate__(self) -> int:
        print("getstate")
        return 0


try:
    copy.copy(Child())
except TypeError:
    print("invalid reducer")
else:
    raise AssertionError("copy accepted an invalid reducer")

try:
    copy.copy(WithState())
except TypeError:
    print("invalid state")
else:
    raise AssertionError("copy accepted invalid state")

try:
    class Keyword(metaclass=abc.ABCMeta, option=True):
        pass
except TypeError:
    print("unexpected subclass keyword")
else:
    raise AssertionError("class accepted an unexpected keyword")
