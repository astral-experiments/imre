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
    check_inherited_inequality()
    child = Child.__new__(Child, path='pkg')
    child.path = 'new'
    assert type(child) is Child and child.path == 'new'
    assert child.__new__ is Child.__new__ is object.__new__
    assert object.__new__.__self__ is object
    assert type(Custom('ok')) is Custom
    assert type(object.__new__(Custom)) is Custom
    assert type(object.__new__(object)) is object

    from collections import UserDict
    from collections.abc import Collection, Container, Iterable, Mapping, MutableMapping, Sized

    for base in (Collection, Container, Iterable, Mapping, MutableMapping, Sized):
        assert base.__new__ is object.__new__

    from collections.abc import Callable

    for abstract in (Mapping, Callable):
        try:
            object.__new__(abstract)
        except TypeError:
            pass
        else:
            raise AssertionError("object.__new__ instantiated an abstract class")

    class Data(UserDict):
        def __init__(self):
            raise AssertionError("explicit allocation called __init__")

    data = Data.__new__(Data)
    assert type(data) is Data and not hasattr(data, "data")

    class MappingList(MutableMapping, list):
        pass

    try:
        object.__new__(MappingList)
    except TypeError as error:
        assert "is not safe" in str(error)
    else:
        raise AssertionError("mapping ABC hid the native list layout")

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


def check_inherited_inequality():
    from collections.abc import Mapping, MutableMapping

    assert Mapping.__ne__ is MutableMapping.__ne__ is object.__ne__
    assert object.__ne__.__objclass__ is object
    calls = []

    class Left:
        __ne__ = MutableMapping.__ne__

        def __eq__(self, other):
            calls.append("left")
            return NotImplemented

    class Right:
        def __eq__(self, other):
            calls.append("right")
            return True

    left, right = Left(), Right()
    assert left.__ne__(right) is NotImplemented
    assert calls == ["left"]
    assert (left != right) is False
    assert calls == ["left", "left", "right"]
    assert object.__ne__(right, left) is False

    class Plain:
        pass

    p = Plain()
    assert object.__ne__(p, p) is False
    assert object.__ne__(p, Plain()) is NotImplemented
    assert (1 != 1.0) is False
    assert (1 != 2.0) is True

    class Recursive:
        __eq__ = object.__ne__

    try:
        object.__ne__(Recursive(), Recursive())
    except RecursionError:
        pass
    else:
        raise AssertionError("unguarded recursive inequality slot")
