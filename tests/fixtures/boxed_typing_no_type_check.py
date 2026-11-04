from typing import get_type_hints, no_type_check


def current():
    class Base:
        def inherited(self, x: int): ...

    @no_type_check
    class Owner(Base):
        field: 1 / 0
        alias = Base.inherited

        class Nested:
            @classmethod
            def method(cls, x: int): ...

        @staticmethod
        def method(x: int): ...

    assert no_type_check(Owner) is Owner
    assert Owner.Nested.method.__func__.__no_type_check__ is True
    assert Owner.method.__no_type_check__ is True
    assert get_type_hints(Owner) == {}
    assert get_type_hints(Base.inherited) == {"x": int}
    assert no_type_check(dict) is dict

    failure = AttributeError("descriptor called")

    class Descriptor:
        def __get__(self, instance, owner):
            raise failure

    class Parent:
        __delattr__ = Descriptor()

    class Child(Parent): ...

    try:
        no_type_check(Child)
    except AttributeError as error:
        assert error is failure
    else:
        raise AssertionError("a user descriptor error was suppressed")

    live_member_lookup()
    print("no_type_check")


def live_member_lookup():
    marker = LookupError("late")
    events = []

    class Raising:
        def __get__(self, instance, owner):
            events.append("Raising")
            raise marker

    class Installer:
        def __get__(self, instance, owner):
            events.append("Installer")
            owner.__delattr__ = Raising()
            return None

    class Base:
        pass

    Base.A = Installer()

    class Child(Base):
        pass

    assert "__delattr__" not in vars(Base) and "__delattr__" not in vars(Child)
    try:
        no_type_check(Child)
    except LookupError as error:
        assert error is marker
    else:
        raise AssertionError("a descriptor installed during no_type_check was skipped")
    assert events == ["Installer", "Raising"]

    class DictChild(dict):
        pass

    assert dict.__delattr__ is object.__delattr__
    assert DictChild.__delattr__ is object.__delattr__
    child = DictChild()
    child.key = 1
    DictChild.__delattr__(child, "key")
    assert "key" not in vars(child)
