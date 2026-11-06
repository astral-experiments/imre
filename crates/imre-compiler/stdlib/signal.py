"""POSIX signal numbers, handlers, and pending signals."""

import _signal
from _signal import NSIG, alarm, default_int_handler, set_wakeup_fd, siginterrupt
from _imre_signal import make_signals as _make_signals
from enum import IntEnum as _IntEnum


# CPython 3.14.5, Lib/signal.py. See stdlib/docstrings/LICENSE.
def _int_to_enum(value, enum_klass):
    """Convert a possible numeric value to an IntEnum member.
    If it's not a known member, return the value itself.
    """
    if not isinstance(value, int):
        return value
    try:
        return enum_klass(value)
    except ValueError:
        return value


Signals = _make_signals()


def sigpending():
    return {_int_to_enum(x, Signals) for x in _signal.sigpending()}


sigpending.__doc__ = _signal.sigpending.__doc__


def valid_signals():
    return {_int_to_enum(x, Signals) for x in _signal.valid_signals()}


valid_signals.__doc__ = _signal.valid_signals.__doc__


class Handlers(_IntEnum):
    """An enumeration."""

    SIG_DFL = _signal.SIG_DFL
    SIG_IGN = _signal.SIG_IGN


globals().update(Signals.__members__)
globals().update(Handlers.__members__)


def _enum_to_int(value):
    try:
        return int(value)
    except (ValueError, TypeError):
        return value


def signal(signalnum, handler):
    previous = _signal.signal(_enum_to_int(signalnum), _enum_to_int(handler))
    return _int_to_enum(previous, Handlers)


def getsignal(signalnum):
    return _int_to_enum(_signal.getsignal(signalnum), Handlers)


signal.__doc__ = _signal.signal.__doc__
getsignal.__doc__ = _signal.getsignal.__doc__
