"""Module documentation.

    Indented module text.
"""
import errno


class Documented:
    """Class documentation.

        Class details.
    """
    captured = __doc__


class Undocumented(Documented):
    captured = __doc__


class Receiver:
    def __init__(self, original):
        self.original = original

    def __get__(self, instance, owner):
        return self.original if instance is None else "instance"


class Replaced:
    "before replacement"
    __doc__ = Receiver(__doc__)


def current():
    print(repr(__doc__), repr(Documented.__doc__), repr(Documented.captured))
    print(repr(Undocumented.__doc__), repr(Undocumented.captured))
    print(repr(Replaced.__doc__), Replaced().__doc__, isinstance(errno.__doc__, str))
