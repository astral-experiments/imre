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

    for args in ((), (42,), (Plain, 1), (Custom, 1), (list,), (NativeList,),
                 (ModuleType,), (Abstract,)):
        try:
            object.__new__(*args)
        except TypeError as exc:
            print(str(exc))
        else:
            raise AssertionError('invalid allocation succeeded')
