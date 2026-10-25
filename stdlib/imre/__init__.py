"""CPython implementations of Imre's typed library boundaries."""

import math as _math
import struct as _binary
from dataclasses import dataclass as _dataclass
from types import FunctionType as _FunctionType
from typing import (
    TypeVar as _TypeVar,
)
from typing import (
    dataclass_transform as _transform,
)
from typing import (
    get_type_hints as _hints,
)

_T = _TypeVar("_T")


def _word(value):
    if type(value) is bool:
        return int(value)
    if value == 0.0:
        return 0
    if _math.isnan(value):
        return 0x7FF8000000000000
    return _binary.unpack("=Q", _binary.pack("=d", value))[0]


@_transform(frozen_default=True)
def struct(cls: type[_T], /) -> type[_T]:
    """Declare a final, frozen value with required float or bool fields."""
    if cls.__bases__ != (object,) or type(cls) is not type:
        raise TypeError("an imre.struct is final and cannot inherit fields")
    fields = tuple(_hints(cls).items())
    if not fields or any(t is not float and t is not bool for _, t in fields):
        raise TypeError("imre.struct requires one or more float or bool fields")
    for name in vars(cls):
        if name in {
            "__module__",
            "__qualname__",
            "__doc__",
            "__annotations__",
            "__annotate__",
            "__firstlineno__",
            "__static_attributes__",
            "__dict__",
            "__weakref__",
            "__annotate_func__",
            "__annotations_cache__",
        }:
            continue
        if name.startswith("__") or not isinstance(vars(cls)[name], _FunctionType):
            raise TypeError(
                "imre.struct supports required fields and ordinary instance methods"
            )
    if any(name.startswith("__") or name in vars(cls) for name, _ in fields):
        raise TypeError("imre.struct fields cannot have defaults or reserved names")

    def check(self):
        for name, annotation in fields:
            if type(getattr(self, name)) is not annotation:
                raise TypeError(f"{name} must be {annotation.__name__}")

    def equal(self, other):
        if type(other) is not type(self):
            return NotImplemented
        return all(
            _word(getattr(self, name)) == _word(getattr(other, name))
            for name, _ in fields
        )

    def value_hash(self):
        h = 0xCBF29CE484222325
        for name, _ in fields:
            h = ((h ^ _word(getattr(self, name))) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
        return h & 0x7FFFFFFFFFFFFFFF

    def final(subclass, **kwargs):
        raise TypeError(f"{cls.__name__} is a final imre.struct")

    cls.__post_init__ = check
    cls.__eq__ = equal
    cls.__hash__ = value_hash
    result = _dataclass(frozen=True, slots=True, eq=False)(cls)
    result.__init_subclass__ = classmethod(final)
    return result
