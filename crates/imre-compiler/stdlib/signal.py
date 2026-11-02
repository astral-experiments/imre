"""POSIX signal numbers, the process alarm, and Linux pending signals."""

import _signal
from _signal import alarm
from enum import IntEnum as _IntEnum


if hasattr(_signal, "sigpending"):
    from _imre_signal import make_signals as _make_signals
    from _signal import NSIG

    Signals = _make_signals()

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


    def sigpending():
        return {_int_to_enum(x, Signals) for x in _signal.sigpending()}


    sigpending.__doc__ = _signal.sigpending.__doc__
else:
    class Signals(_IntEnum):
        SIGINT = _signal.SIGINT
        SIGALRM = _signal.SIGALRM
        SIGTERM = _signal.SIGTERM


globals().update(Signals.__members__)
