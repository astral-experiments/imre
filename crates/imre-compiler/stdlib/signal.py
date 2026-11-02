"""POSIX signal numbers and the process alarm."""

import _signal
from _signal import alarm
from enum import IntEnum as _IntEnum


class Signals(_IntEnum):
    SIGINT = _signal.SIGINT
    SIGALRM = _signal.SIGALRM
    SIGTERM = _signal.SIGTERM


SIGINT = Signals.SIGINT
SIGALRM = Signals.SIGALRM
SIGTERM = Signals.SIGTERM
