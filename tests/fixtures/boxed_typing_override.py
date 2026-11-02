from inspect import getattr_static
from typing import override


def current():
    def method(cls):
        return cls

    class C:
        marked = override(classmethod(method))

    wrapper = getattr_static(C, "marked")
    assert wrapper.__override__ is True
    assert C.marked() is C and not hasattr(method, "__override__")

    try:
        override(method=method)
    except TypeError:
        pass
    else:
        raise AssertionError("override accepted a keyword argument")
    assert not hasattr(method, "__override__")

    class Refuses:
        def __setattr__(self, name, value):
            assert name == "__override__" and value is True
            raise ValueError("refused")

    try:
        override(Refuses())
    except ValueError as error:
        assert str(error) == "refused"
    else:
        raise AssertionError("override suppressed the setter error")
    assert override(int) is int
    assert override(method) is method and method.__override__ is True
    print("override decorator")


current()
