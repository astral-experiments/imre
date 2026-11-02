import typing as t
from typing import SupportsAbs, SupportsInt


class Named(t.Protocol):
    def name(self): ...


@t.runtime_checkable
class HasName(Named, t.Protocol):
    pass


class Item:
    def name(self):
        return "item"


class Number(SupportsInt):
    def __int__(self):
        return 42


print(isinstance(Item(), HasName), isinstance(object(), HasName))
try:
    t.runtime_checkable(Item)
except TypeError as exc:
    print(type(exc).__name__)


assert SupportsInt is t.SupportsInt
assert t.get_protocol_members(SupportsInt) == {"__int__"}
assert SupportsInt.__abstractmethods__ == {"__int__"}
assert not t.is_protocol(Number) and isinstance(Number(), SupportsInt)
assert SupportsAbs[int].__origin__ is SupportsAbs
parameter, = SupportsAbs.__type_params__
assert SupportsAbs.__abs__.__annotations__["return"] is parameter
