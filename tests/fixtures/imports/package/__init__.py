"""Package initialization has only static definitions and imports."""

from .operations import twice as adjust
from . import nested


def value(number: int) -> int:
    return adjust(number)
