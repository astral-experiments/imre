import abc


def current():
    events = []

    class Descriptor:
        def __set_name__(self, owner, name):
            events.append((owner.__name__, name))

    class Parent(abc.ABC):
        @abc.abstractmethod
        def read(self):
            pass

        def __init_subclass__(cls, *, marker=None):
            events.append((cls.__name__, marker))

    namespace = {"descriptor": Descriptor(), "__slots__": ("field",)}
    incomplete = type("Incomplete", (Parent,), namespace, marker=11)
    assert type(incomplete) is abc.ABCMeta
    assert incomplete.__abstractmethods__ == frozenset({"read"})
    assert events == [("Incomplete", "descriptor"), ("Incomplete", 11)]
    assert "__module__" not in namespace
    try:
        incomplete()
    except TypeError:
        pass
    else:
        raise AssertionError("constructed a class with abstract read")

    def read(self):
        return self.field

    concrete = abc.ABCMeta("Concrete", (incomplete,), {"read": read})
    assert concrete.__bases__ == (incomplete,)
    assert concrete.__mro__[1:3] == (incomplete, Parent)
    value = concrete()
    value.field = 42
    assert value.read() == 42

    slotted = type.__new__(type, "Slotted", (), {"__slots__": ("value",)})
    instance = slotted()
    instance.value = 13
    assert instance.value == 13 and not hasattr(instance, "__dict__")

    def invalid():
        return type("Invalid", [], {})

    try:
        invalid()
    except TypeError as exc:
        assert str(exc) == "type.__new__() argument 2 must be tuple, not list"
    else:
        raise AssertionError("constructed with invalid bases")
