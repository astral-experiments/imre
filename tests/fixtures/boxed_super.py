class Name:
    def __get__(self, obj, owner):
        return owner.__name__


class A:
    label = Name()

    def walk(self):
        return "A"


class B(A):
    def walk(self):
        return super().walk() + "B"


class C(A):
    def walk(self):
        return super().walk() + "C"


class D(C, B):
    def walk(self):
        return super().walk() + "D"

    def saved(self):
        return super()

    def enclosing(self):
        def nested(receiver):
            return __class__ is D, super().walk()
        return nested(self)

    def captured(self, replacement):
        def receiver():
            return self

        original = super().__self__ is receiver()
        self = replacement
        rebound = super().__self__ is replacement and receiver() is replacement
        del self
        try:
            super()
        except RuntimeError as error:
            return original, rebound, error.args

    def deleted(self):
        try:
            raise ValueError()
        except ValueError as self:
            pass
        return super()


class Proxy:
    @property
    def __class__(self):
        return D


def outside(value):
    return super()


class Mapping(dict):
    def __init__(self, pairs):
        super().__init__(pairs)

    def write(self, name, value):
        super().__setitem__(name, value)


def current():
    obj = D()
    obj.label = "instance"
    print(obj.walk(), D.walk(obj), obj.enclosing())
    print(obj.captured(D()))
    proxy = obj.saved()
    for _ in range(6000):
        []
    print(proxy.walk(), obj.label, proxy.label, proxy.__thisclass__ is D, proxy.__self__ is obj)
    print(type(proxy) is super, super(int, bool).__self_class__ is bool, super(int, True).__self_class__ is bool)
    print(proxy.__self_class__ is D, super(C, D).walk(obj), super(C).__self__ is None)
    mapping = Mapping([("visits", 1)])
    mapping.write("visits", mapping["visits"] + 1)
    print(mapping["visits"], len(mapping))
    wrapped = Proxy()
    print(super(D, wrapped).__self__ is wrapped, super(D, wrapped).label)
    for call in (lambda: outside(obj), obj.deleted, lambda: super(C, A())):
        try:
            call()
        except (RuntimeError, TypeError) as error:
            print(str(error))
    try:
        class Duplicate(A, A):
            pass
    except TypeError as error:
        print(str(error))
    try:
        class Inconsistent(A, D):
            pass
    except TypeError as error:
        print(str(error))

    check_mro()
    check_subclass_hooks()


def check_mro():
    original = D.mro()
    assert original == [D, C, B, A, object] and D.__mro__ == tuple(original)
    original.clear()
    assert D.mro() == type.mro(D) == [D, C, B, A, object]
    from collections import defaultdict
    from datetime import timezone, tzinfo
    from os import terminal_size
    from types import ModuleType
    assert bool.mro() == type.mro(bool) == [bool, int, object]
    assert terminal_size.mro() == [terminal_size, tuple, object]
    assert type.mro(defaultdict) == [defaultdict, dict, object]
    assert timezone.mro() == [timezone, tzinfo, object]
    assert D.mro.__self__ is D and not hasattr(super(D, D()), "mro")

    class Meta(type): pass
    class ViaMeta(metaclass=Meta): pass
    class Shadow(D): mro = None
    assert ViaMeta.mro() == [ViaMeta, object] and Meta.mro is type.mro
    assert ViaMeta.mro.__qualname__ == ViaMeta.__qualname__ + ".mro"
    assert type.mro(Meta) == [Meta, type, object]
    assert Shadow.mro is None and type.mro(Shadow)[1:] == D.mro()
    class NamedModule(ModuleType): pass
    NamedModule.__name__ = "type"
    fake = NamedModule("fake")
    for invalid in (lambda: type.mro(fake), lambda: type.mro(D()), lambda: D.mro(1), lambda: Meta.mro()):
        try:
            invalid()
        except TypeError:
            pass
        else:
            raise AssertionError("invalid mro call succeeded")


def check_subclass_hooks():
    events = []

    class Root:
        def __init_subclass__(cls):
            events.append(("root", cls.__name__))
            super().__init_subclass__()

    class Left(Root):
        def __init_subclass__(cls):
            events.append(("left", cls.__name__))
            super().__init_subclass__()

    class Right(Root):
        @classmethod
        def __init_subclass__(cls):
            events.append(("right", cls.__name__))
            super().__init_subclass__()

    class Field:
        def __set_name__(self, owner, name):
            events.append((name, owner.__name__))

    def decorate(cls):
        events.append(("decorator", cls.__name__))
        return cls

    @decorate
    class Child(Left, Right):
        field = Field()

    assert events == [
        ("root", "Left"), ("root", "Right"),
        ("field", "Child"), ("left", "Child"), ("right", "Child"),
        ("root", "Child"), ("decorator", "Child"),
    ]
    assert isinstance(Left.__dict__["__init_subclass__"], classmethod)
    assert Child.__init_subclass__.__self__ is Child

    class Refuse:
        @staticmethod
        def __init_subclass__():
            raise TypeError("refused")

    retained = Child
    try:
        class retained(list, Refuse):
            pass
    except TypeError as error:
        assert error.args == ("refused",)
    assert retained is Child

    class Hook:
        def __get__(self, obj, owner):
            assert obj is None
            return lambda: events.append(("descriptor", owner.__name__))

    class Custom:
        __init_subclass__ = Hook()

    class CustomChild(Custom):
        pass

    assert events[-1] == ("descriptor", "CustomChild")

    def replacement():
        events.append("rebound")

    Custom.__init_subclass__ = replacement
    class Rebound(Custom):
        pass
    assert events[-1] == "rebound"
    assert object.__init_subclass__.__self__ is object
    assert (2).__init_subclass__.__self__ is int
    try:
        Rebound.__init_subclass__(1)
    except TypeError:
        pass
    else:
        raise AssertionError("rebound hook unexpectedly accepted an argument")
