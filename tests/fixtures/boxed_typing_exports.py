from typing import *
import typing as _typing
from typing import __all__ as _exports

_public_names = [name for name in globals() if not name.startswith("_")]


def current():
    assert _exports is _typing.__all__
    assert _public_names == _exports
    for name in _exports:
        assert globals()[name] is getattr(_typing, name)
    print(_exports)

    assert _typing.evaluate_forward_ref(_typing.ForwardRef("int")) is int
    import doctest

    print([case.id() for case in doctest.DocTestSuite(_typing)])

    from types import MappingProxyType
    namespace = vars(Any)
    assert type(namespace) is MappingProxyType
    assert namespace["__module__"] == Any.__module__ == "typing"
    assert namespace["__doc__"] == Any.__doc__ and Any.__doc__.startswith("Special type")
    assert type(namespace["__new__"]) is staticmethod
    assert namespace["__new__"].__func__ is Any.__new__
    assert namespace["__dict__"].__objclass__ is Any
    assert namespace["__weakref__"].__objclass__ is Any

    class Child(Any):
        pass

    child = Child()
    assert type(child) is Child
    descriptor = namespace["__dict__"]
    assert descriptor.__get__(None, Child) is descriptor
    assert descriptor.__get__(child, Child) is vars(child)
    child.value = 1
    assert vars(child) == {"value": 1}

    marker = object()

    class Tail:
        def __new__(cls):
            return marker

    class Joint(Any, Tail):
        pass

    assert Any.__new__(Joint) is marker
