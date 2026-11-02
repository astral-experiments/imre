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

    check_collection_flags()


def check_collection_flags():
    captured = []

    class Base(abc.ABC):
        def __init_subclass__(cls):
            captured.append((cls, cls.__dict__["__abc_tpflags__"]))

    for mask, expected in ((32 | 64, TypeError), (-1, TypeError), (1 << 100, OverflowError)):
        namespace = {"__abc_tpflags__": mask}
        try:
            abc.ABCMeta("Invalid", (Base,), namespace)
        except expected:
            pass
        else:
            raise AssertionError("ABCMeta accepted invalid collection flags")
        cls, observed = captured[-1]
        assert observed == mask and namespace["__abc_tpflags__"] == mask
        assert "__abc_tpflags__" not in cls.__dict__ and cls.__abstractmethods__ == frozenset()

    class IntSubclass(int):
        pass

    class Index:
        def __index__(self):
            raise AssertionError("ABCMeta called __index__")

    for mask in (0, 32, 64, True, IntSubclass(96), Index()):
        cls = abc.ABCMeta("Valid", (Base,), {"__abc_tpflags__": mask})
        assert captured[-1] == (cls, mask)

    class Plain:
        __abc_tpflags__ = 96

    class Inherited(Plain, abc.ABC):
        pass

    assert Inherited.__abc_tpflags__ == 96
