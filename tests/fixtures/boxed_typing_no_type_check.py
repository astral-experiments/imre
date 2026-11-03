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

    class Descriptor:
        def __get__(self, instance, owner):
            raise AttributeError("descriptor called")

    class Parent:
        __delattr__ = Descriptor()

    class Child(Parent): ...

    try:
        no_type_check(Child)
    except AttributeError as error:
        assert str(error) == "descriptor called"
    else:
        raise AssertionError("a user descriptor error was suppressed")
    print("no_type_check")
