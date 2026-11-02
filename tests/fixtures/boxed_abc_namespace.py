import abc


def current():
    calls = []

    class Base(abc.ABC):
        @abc.abstractmethod
        def read(self):
            pass

        def __init_subclass__(cls, *, marker):
            calls.append(("subclass", type(cls), marker))

    class Field:
        def __set_name__(self, owner, name):
            calls.append(("field", type(owner), owner.__module__, name))

    namespace = {"field": Field()}
    abstract = type("Derived", (Base,), namespace, marker=3)
    assert abstract.__abstractmethods__ == frozenset({"read"})
    assert calls == [("field", abc.ABCMeta, "abc", "field"), ("subclass", abc.ABCMeta, 3)]
    assert "__module__" not in namespace
    try:
        abstract()
    except TypeError:
        pass
    else:
        raise AssertionError("abstract method did not prevent instantiation")

    concrete = abc.ABCMeta("Concrete", (abstract,), {"read": lambda self: 42}, marker=4)
    assert concrete.__module__ == "abc"
    assert type(concrete) is abc.ABCMeta
    assert concrete.__mro__ == (concrete, abstract, Base, abc.ABC, object)
    assert not concrete.__abstractmethods__ and concrete().read() == 42
    assert calls[-1] == ("subclass", abc.ABCMeta, 4)
