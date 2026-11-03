from __future__ import annotations

from types import CodeType
from typing import Unpack, get_type_hints


Item = bytes


def make_method():
    class Owner:
        Item = str

        def method[Item, *Items, **P](self, first: Item, *rest: *Items, **options: P.kwargs):
            return Item

    return Owner.method


method = make_method()
other = make_method()
item, items, params = method.__type_params__
assert get_type_hints(method) == {"first": item, "rest": Unpack[items], "options": params.kwargs}
assert method(None, None) is item
assert method.__globals__ is globals() and Item is bytes
assert isinstance(method.__code__, CodeType) and method.__code__ is other.__code__
# CO_FUTURE_ANNOTATIONS, CO_NESTED, CO_VARKEYWORDS, CO_VARARGS, CO_NEWLOCALS, CO_OPTIMIZED.
assert method.__code__.co_flags == 0x100001F
assert method.__code__.co_freevars == ("Item",)
assert method.__closure__[0].cell_contents is item
assert other.__type_params__[0] is not item
