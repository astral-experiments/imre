import typing
from typing import override


def current():
    assert override is typing.override

    class Base:
        def method(self):
            return 1

    class Child(Base):
        @override
        def method(self):
            return 2

        @classmethod
        @override
        def factory(cls):
            return cls()

        @override
        @property
        def value(self):
            return 3

    assert not hasattr(Base.method, "__override__")
    assert Child().method() == 2 and Child().method.__override__ is True
    assert type(Child.factory()) is Child and Child.factory.__override__ is True
    assert Child().value == 3 and not hasattr(Child.value.fget, "__override__")
    assert override(int) is int

    class Rejected:
        def __setattr__(self, name, value):
            assert name == "__override__" and value is True
            raise ValueError("marker rejected")

    try:
        override(Rejected())
    except ValueError as error:
        assert error.args == ("marker rejected",)
    else:
        raise AssertionError("suppressed a custom setter exception")

    try:
        override(method=Base.method)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a keyword for the positional-only method")
    print("override decorator")


if __name__ == "__main__":
    current()
