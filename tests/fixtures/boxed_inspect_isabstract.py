import abc
import inspect
from inspect import isabstract


def current():
    observed = []

    class Base(abc.ABC):
        def __init_subclass__(cls):
            observed.append(inspect.isabstract(cls))

    class Abstract(Base):
        @abc.abstractmethod
        def method(self):
            pass

    class Inherited(Abstract):
        pass

    class Concrete(Inherited):
        def method(self):
            return 1

    assert observed == [True, True, False]
    assert isabstract is inspect.isabstract
    assert isabstract(Inherited)
    assert not inspect.isabstract(Concrete)
    assert not inspect.isabstract(Concrete())
    assert not inspect.isabstract(int)

    class Plain:
        pass

    methods = {"missing"}
    Plain.__abstractmethods__ = methods
    methods.clear()
    assert inspect.isabstract(Plain)
    Plain.__abstractmethods__ = methods
    methods.add("new")
    assert not inspect.isabstract(Plain)
    __abstractmethods__ = methods

    class Captured:
        outer = __abstractmethods__

    assert Captured.outer is methods
    assert not hasattr(Captured, "__abstractmethods__")
    check_flag_overrides()
    check_descriptor_binding()
    check_declaration_snapshot()
    print("abstract class flags and subclass hooks")


def check_flag_overrides():
    class Shadow:
        __flags__ = inspect.TPFLAGS_IS_ABSTRACT

    assert not inspect.isabstract(Shadow)

    class Hook(type):
        pass

    class Late(metaclass=Hook):
        pass

    def read(cls, name):
        if name == "__flags__":
            return inspect.TPFLAGS_IS_ABSTRACT
        return type.__getattribute__(cls, name)

    Hook.__getattribute__ = read
    assert inspect.isabstract(Late)

    class Mask(abc.ABCMeta):
        __flags__ = 0

    class Masked(metaclass=Mask):
        @abc.abstractmethod
        def method(self):
            pass

    assert Masked.__abstractmethods__ == {"method"}
    assert not inspect.isabstract(Masked)

    masks = []

    class Operand:
        def __and__(self, mask):
            masks.append(mask)
            return True

    class Meta(type):
        @property
        def __flags__(cls):
            if cls.operand is None:
                raise LookupError("flags getter")
            return cls.operand

    class Flagged(metaclass=Meta):
        __flags__ = 0
        operand = Operand()

    assert inspect.isabstract(Flagged)
    assert masks == [1 << 20]
    Flagged.operand = "invalid"
    try:
        inspect.isabstract(Flagged)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted nonnumeric flags")
    Flagged.operand = None
    try:
        inspect.isabstract(Flagged)
    except LookupError as exc:
        assert exc.args == ("flags getter",)
    else:
        raise AssertionError("ignored metaclass exception")


def check_descriptor_binding():
    seen = []

    class Descriptor:
        __isabstractmethod__ = True

        def __get__(self, instance, owner):
            return 1

    class Observer:
        def __init_subclass__(cls):
            seen.append(inspect.isabstract(cls))

    class Base(Observer, abc.ABC):
        action = Descriptor()

    class Child(Base):
        pass

    assert seen == [True, False]
    assert Base.__abstractmethods__ == {"action"} and inspect.isabstract(Base)
    assert not Child.__abstractmethods__ and not inspect.isabstract(Child)
    assert Child().action == 1


def check_declaration_snapshot():
    class Remove:
        def __set_name__(self, owner, name):
            self.owner = owner

        @property
        def __isabstractmethod__(self):
            del self.owner.second
            return True

    class Abstract:
        __isabstractmethod__ = True

    class Subject(abc.ABC):
        first = Remove()
        second = Abstract()

    assert Subject.__abstractmethods__ == {"first", "second"}
