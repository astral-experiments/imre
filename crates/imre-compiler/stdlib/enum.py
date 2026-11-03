"""Global enum helpers from CPython 3.14.5 Lib/enum.py. See stdlib/docstrings/LICENSE."""

import sys as _sys
from enum import Flag as _Flag, IntEnum as _IntEnum, IntFlag as _IntFlag, StrEnum as _StrEnum


def _install_flag_methods(methods):
    # The native enum base owns member construction and the value cache.
    for name in ("__contains__", "__iter__", "__len__", "__str__", "__bool__",
                 "_get_value", "__or__", "__and__", "__xor__", "__invert__",
                 "__ror__", "__rand__", "__rxor__"):
        setattr(_Flag, name, vars(methods)[name])
    return _Flag

@_install_flag_methods
class Flag:
    def __contains__(self, other):
        """
        Returns True if self has at least the same flags set as other.
        """
        if not isinstance(other, self.__class__):
            raise TypeError(
                "unsupported operand type(s) for 'in': %r and %r" % (
                    type(other).__qualname__, self.__class__.__qualname__))
        return other._value_ & self._value_ == other._value_

    def __iter__(self):
        bits = self._value_ & self._flag_mask_
        if bits < 0:
            raise ValueError(f"{bits!r} is not a positive integer")
        members = []
        while bits:
            bit = bits & -bits
            members.append(self._value2member_map_.get(bit))
            bits ^= bit
        if self._flag_definition_order:
            members.sort(key=lambda member: member._sort_order_)
        yield from members

    def __len__(self):
        # Count the set bits without allocating a textual representation.
        bits = abs(self._value_)
        count = 0
        while bits:
            bits &= bits - 1
            count += 1
        return count

    def __str__(self):
        cls_name = self.__class__.__name__
        if self._name_ is None:
            return '%s(%r)' % (cls_name, self._value_)
        else:
            return "%s.%s" % (cls_name, self._name_)

    def __bool__(self):
        return bool(self._value_)

    def _get_value(self, flag):
        if isinstance(flag, self.__class__):
            return flag._value_
        elif self._member_type_ is not object and isinstance(flag, self._member_type_):
            return flag
        return NotImplemented

    def __or__(self, other):
        other_value = self._get_value(other)
        if other_value is NotImplemented:
            return NotImplemented

        for flag in self, other:
            if self._get_value(flag) is None:
                raise TypeError(f"'{flag}' cannot be combined with other flags with |")
        value = self._value_
        return self.__class__(value | other_value)

    def __and__(self, other):
        other_value = self._get_value(other)
        if other_value is NotImplemented:
            return NotImplemented

        for flag in self, other:
            if self._get_value(flag) is None:
                raise TypeError(f"'{flag}' cannot be combined with other flags with &")
        value = self._value_
        return self.__class__(value & other_value)

    def __xor__(self, other):
        other_value = self._get_value(other)
        if other_value is NotImplemented:
            return NotImplemented

        for flag in self, other:
            if self._get_value(flag) is None:
                raise TypeError(f"'{flag}' cannot be combined with other flags with ^")
        value = self._value_
        return self.__class__(value ^ other_value)

    def __invert__(self):
        value = self._value_
        if value is None:
            raise TypeError(f"'{self}' cannot be inverted")
        if isinstance(self, _IntFlag):
            return self.__class__(~value)
        return self.__class__(self._singles_mask_ & ~value)

    __ror__ = __or__
    __rand__ = __and__
    __rxor__ = __xor__


def global_enum_repr(self):
    """
    use module.enum_name instead of class.enum_name

    the module is the last module in case of a multi-module name
    """
    module = self.__class__.__module__.split('.')[-1]
    return '%s.%s' % (module, self._name_)

def global_flag_repr(self):
    """
    use module.flag_name instead of class.flag_name

    the module is the last module in case of a multi-module name
    """
    module = self.__class__.__module__.split('.')[-1]
    cls_name = self.__class__.__name__
    if self._name_ is None:
        return "%s.%s(%r)" % (module, cls_name, self._value_)
    if self._value_ and not self._value_ & (self._value_ - 1):
        return '%s.%s' % (module, self._name_)
    # Imre's native IntFlag uses the KEEP boundary.
    return '|'.join(name if name[0].isdigit() else '%s.%s' % (module, name)
                    for name in self._name_.split('|'))


def global_str(self):
    """
    use enum_name instead of class.enum_name
    """
    if self._name_ is None:
        cls_name = self.__class__.__name__
        return "%s(%r)" % (cls_name, self._value_)
    else:
        return self._name_

def global_enum(cls, update_str=False):
    """
    decorator that makes the repr() of an enum member reference its module
    instead of its class; also exports all members to the enum's module's
    global namespace
    """
    if issubclass(cls, _Flag):
        cls.__repr__ = global_flag_repr
    else:
        cls.__repr__ = global_enum_repr
    if not issubclass(cls, (_IntEnum, _IntFlag, _StrEnum)) or update_str:
        cls.__str__ = global_str
    _sys.modules[cls.__module__].__dict__.update(cls.__members__)
    return cls


_not_given = object()


def _functional_members(base, names, start):
    from enum import auto
    if isinstance(names, str):
        names = names.replace(',', ' ').split()
    if isinstance(names, (list, tuple)) and names and isinstance(names[0], str):
        for i, name in enumerate(names):
            if i or base is _StrEnum:
                yield name, auto()
            elif start is None and base in (_Flag, _IntFlag):
                yield name, 1
            else:
                yield name, start
    elif names is not None:
        for item in names:
            if isinstance(item, str):
                yield item, names[item]
            else:
                name, value = item
                yield name, value


def _prepare_functional_enum(base, value, names=_not_given, *values, module=None,
                             qualname=None, type=None, start=1, boundary=None):
    if type is not None or boundary is not None:
        raise NotImplementedError("functional enum mixins and boundary options are unsupported")
    if names is _not_given:
        raise TypeError(f"{base} has no members; specify `names=()` if you meant to create a new, empty, enum")
    return value, _functional_members(base, names, start), module, qualname
