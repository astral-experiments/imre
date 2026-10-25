"""The re-exported child reads metadata already initialized by its parent."""

__all__ = ["adjust"]

from .operations import twice as adjust
from . import nested


def value(number: int) -> int:
    return adjust(number)
