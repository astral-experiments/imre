import typing
from typing import final


def current():
    def function(): ...

    class Marked: ...

    assert typing.final(Marked) is Marked

    assert final(f=function) is function
    assert function.__final__ is True and Marked.__final__ is True
    assert final(int) is int and not hasattr(int, "__final__")

    class Slotted:
        __slots__ = ()

    obj = Slotted()
    assert final(obj) is obj and not hasattr(obj, "__final__")

    class ReadOnly:
        @property
        def __final__(self):
            return False

    obj = ReadOnly()
    assert final(obj) is obj and obj.__final__ is False

    class Reject:
        def __setattr__(self, name, value):
            assert name == "__final__" and value is True
            raise ValueError("reject marker")

    try:
        final(Reject())
    except ValueError as error:
        assert str(error) == "reject marker"
    else:
        raise AssertionError("unrelated attribute error suppressed")
    print("final decorator")


if __name__ == "__main__":
    current()
