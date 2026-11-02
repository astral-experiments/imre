def current():
    from abc import ABC, abstractmethod
    from types import ModuleType

    class Plain:
        pass

    class Loader:
        def __init__(self, path):
            raise AssertionError("explicit allocation must not initialize")

    class Child(Loader):
        __slots__ = ('path',)

    class Custom:
        def __new__(cls, value):
            return super().__new__(cls)

    class NativeList(list):
        pass

    class Abstract(ABC):
        @abstractmethod
        def load(self):
            pass

    check_default_state()
    child = Child.__new__(Child, path='pkg')
    child.path = 'new'
    assert type(child) is Child and child.path == 'new'
    assert child.__new__ is Child.__new__ is object.__new__
    assert object.__new__.__self__ is object
    assert type(Custom('ok')) is Custom
    assert type(object.__new__(Custom)) is Custom
    assert type(object.__new__(object)) is object

    from typing import Generic, Protocol, TypeVar

    parameter = TypeVar("parameter")

    class Marker(Protocol[parameter]):
        pass

    class Concrete(Marker[parameter]):
        def __new__(cls, *args):
            return super().__new__(cls, *args)

    assert type(Concrete[int]()) is Concrete
    for base in (Generic, Protocol, Marker):
        assert type(object.__new__(base)) is base

    class ListMarker(Marker, list):
        pass

    try:
        object.__new__(ListMarker)
    except TypeError as exc:
        assert str(exc) == "object.__new__(ListMarker) is not safe, use ListMarker.__new__()"
    else:
        raise AssertionError("object.__new__ allocated a list subclass")

    for args in ((), (42,), (Plain, 1), (Custom, 1), (list,), (NativeList,),
                 (ModuleType,), (Abstract,)):
        try:
            object.__new__(*args)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError('invalid allocation succeeded')


def check_default_state():
    class Slots:
        __slots__ = ("__private", "missing")

    class Mixed(Slots):
        def __getattribute__(self, name):
            if name == "__dict__":
                raise AssertionError("read the public __dict__")
            return super().__getattribute__(name)

    obj = Mixed()
    assert obj.__getstate__() is None
    obj._Slots__private = 3
    obj.extra = [4]
    state, slots = obj.__getstate__()
    assert state is object.__getattribute__(obj, "__dict__")
    assert state == {"extra": [4]} and slots == {"_Slots__private": 3}
    assert slots is not obj.__getstate__()[1]
    assert Mixed.__dict__["__slotnames__"] == ["_Slots__private", "missing"]

    class Changing:
        __slotnames__ = ["value"]

        @property
        def value(self):
            self.__slotnames__.append("next")
            return 7

    try:
        object.__getstate__(Changing())
    except RuntimeError as error:
        assert str(error) == "__slotnames__ changed size during iteration"
    else:
        raise AssertionError("ignored a slot cache size change")

    class Custom(Mixed):
        def __getstate__(self):
            return super().__getstate__(), "custom"

    assert Custom().__getstate__() == (None, "custom")
