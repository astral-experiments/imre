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
