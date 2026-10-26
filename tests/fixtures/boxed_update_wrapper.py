from __future__ import annotations
from functools import WRAPPER_ASSIGNMENTS, WRAPPER_UPDATES, update_wrapper
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


def alternate(fmt):
    return {"format": fmt}


wrapper.__annotate__ = alternate
print(wrapper.__annotations__, wrapper.__annotations__ is wrapper.__annotations__)
wrapper.__annotations__ = {"cached": True}
print(wrapper.__annotate__, wrapper.__annotations__)
