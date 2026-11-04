from __future__ import annotations
from functools import WRAPPER_ASSIGNMENTS, WRAPPER_UPDATES, update_wrapper, wraps
from inspect import getdoc
from sys import path


class Store:
    def put(self, key: K, value: V) -> None:
        """Save an item."""


def decorate(original):
    def wrapper(*args):
        return original(*args)

    wrapper.local = 9
    print(wrapper.__qualname__)
    return update_wrapper(wrapper, original)


Store.put.tag = "method"
Store.put.__wrapped__ = 12
wrapper = decorate(Store.put)
print(WRAPPER_ASSIGNMENTS, WRAPPER_UPDATES)
print(wrapper.__module__, wrapper.__name__, wrapper.__qualname__, wrapper.__doc__)
print(wrapper.tag, wrapper.local, wrapper.__wrapped__ is Store.put)
print(wrapper.__annotate__ is Store.put.__annotate__, wrapper.__annotations__, wrapper.__type_params__)
print(wrapper.__dict__["__wrapped__"] is Store.put, "__doc__" in wrapper.__dict__)


class Target:
    info = {"kept": 1}


class Source:
    info = {"added": 2}


target = Target()
print(update_wrapper(wrapped=Source, wrapper=target, assigned=("missing",), updated=("info",)) is target)
print(target.info, target.__wrapped__ is Source)


def assigned():
    yield "info"
    assert target.info is Source.info


update_wrapper(target, Source, assigned(), ())


def alternate(fmt):
    return {"format": fmt}


wrapper.__annotate__ = alternate
print(wrapper.__annotations__, wrapper.__annotations__ is wrapper.__annotations__)
wrapper.__annotations__ = {"cached": True}
print(wrapper.__annotate__, wrapper.__annotations__)


class Annotations(dict):
    pass


annotations = Annotations({"value": int})
wrapper.__annotations__ = annotations
print(wrapper.__annotations__ is annotations, isinstance(wrapper.__annotations__, Annotations))


def subclass_annotations(fmt):
    return Annotations({"format": fmt})


wrapper.__annotate__ = subclass_annotations
lazy_annotations = wrapper.__annotations__
print(isinstance(lazy_annotations, Annotations), lazy_annotations["format"])


class FunctionNamespace(dict):
    def __getitem__(self, key):
        raise AssertionError("attribute lookup called __getitem__")

    def __setitem__(self, key, value):
        raise AssertionError("attribute assignment called __setitem__")


namespace = FunctionNamespace(existing=7)
wrapper.__dict__ = namespace
wrapper.added = 8
print(wrapper.__dict__ is namespace, wrapper.existing, dict.__getitem__(namespace, "added"))


def original(value):
    """Original docstring."""
    return value + 1


original.tag = "copied"
decorate = wraps(wrapped=original)
print(decorate.func is update_wrapper, decorate.args, decorate.keywords["wrapped"] is original)
print(decorate.keywords["assigned"] is WRAPPER_ASSIGNMENTS, decorate.keywords["updated"] is WRAPPER_UPDATES)


@decorate
def first(value):
    return 2 * original(value)


@wraps(original, ("__doc__",), updated=())
def second(value):
    return -original(value)


print(first(2), first.__wrapped__ is original, first.__name__, first.tag)
print(second(2), second.__wrapped__ is original, second.__name__, second.__doc__, hasattr(second, "tag"))


@wraps(first)
def outer(value):
    return first(value) + 1


assert outer.__wrapped__ is first
assert outer.__wrapped__.__wrapped__ is original
assert outer(2) == 7


for extremum in (min, max):
    @wraps(extremum)
    def wrapped_extremum(*args, **kwargs):
        return extremum(*args, **kwargs)

    doc = extremum.__doc__
    assert doc.startswith(extremum.__name__ + "(")
    assert wrapped_extremum.__doc__ == getdoc(wrapped_extremum) == getdoc(extremum) == doc
    assert wrapped_extremum.__wrapped__ is extremum
    print(wrapped_extremum([2, 5]), wrapped_extremum([], default=-1), doc)
