"""Global enum helpers from CPython 3.14.5 Lib/enum.py. See stdlib/docstrings/LICENSE."""

import sys as _sys
from enum import IntEnum as _IntEnum, IntFlag as _IntFlag, StrEnum as _StrEnum


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
    if issubclass(cls, _IntFlag):
        cls.__repr__ = global_flag_repr
    else:
        cls.__repr__ = global_enum_repr
    if not issubclass(cls, (_IntEnum, _IntFlag, _StrEnum)) or update_str:
        cls.__str__ = global_str
    _sys.modules[cls.__module__].__dict__.update(cls.__members__)
    return cls
