import weakref
from typing import Any


def current():
    class Plain:
        pass

    class AnyChild(Any):
        __slots__ = ()

    class Unslotted(Any):
        pass

    class PlainChild(Plain):
        __slots__ = ()

    native = vars(object)["__class__"]
    assert native.__objclass__ is object
    assert native.__get__(None, object) is native

    for base, child in ((Any, AnyChild), (Any, Unslotted), (Plain, PlainChild)):
        obj = child()
        assert native.__get__(obj, child) is child
        descriptor = vars(base)["__weakref__"]
        assert descriptor.__objclass__ is base
        assert descriptor.__get__(None, child) is descriptor
        assert descriptor.__get__(obj, child) is None

        foreign = object()
        for operation, args, expected in (
            (descriptor.__get__, (foreign, child), TypeError),
            (descriptor.__set__, (foreign, None), TypeError),
            (descriptor.__delete__, (foreign,), TypeError),
            (descriptor.__set__, (obj, None), AttributeError),
            (descriptor.__delete__, (obj,), AttributeError),
        ):
            try:
                operation(*args)
            except expected as error:
                assert "'__weakref__'" in str(error) and f"'{base.__name__}'" in str(error)
            else:
                raise AssertionError(f"{base.__name__} descriptor accepted {operation.__name__}")

        assert "__dict__" not in vars(child) and "__weakref__" not in vars(child)
        obj.value = 42
        assert vars(base)["__dict__"].__get__(obj, child) is vars(obj)
        assert vars(obj) == {"value": 42}
        handle = weakref.ref(obj)
        assert handle() is obj
        assert descriptor.__get__(obj, child) is obj.__weakref__ is handle
