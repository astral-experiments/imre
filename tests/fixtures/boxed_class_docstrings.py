"""Module documentation.

    Indented module text.
"""
import errno
from inspect import getdoc, ismemberdescriptor


reads = []


class Documented:
    """Class documentation.

        Class details.
    """
    captured = __doc__

    def method(self):
        """Method documentation."""

    @property
    def reading(self):
        """Property documentation."""
        reads.append(self)
        raise AssertionError("property was evaluated")


class Undocumented(Documented):
    captured = __doc__

    def method(self):
        pass

    def _read(self):
        reads.append(self)
        raise AssertionError("property was evaluated")

    reading = property(_read)


class Slots:
    __slots__ = {"power": "measured in kilowatts", "unused": None}


class Receiver:
    def __init__(self, original):
        self.original = original

    def __get__(self, instance, owner):
        return self.original if instance is None else "instance"


class Replaced:
    "before replacement"
    __doc__ = Receiver(__doc__)


def current():
    for value in ("text", 42, True, (1,), frozenset({3})):
        assert value.__doc__ == type(value).__doc__
        assert isinstance(value.__doc__, str) and value.__doc__
    print(repr(__doc__), repr(Documented.__doc__), repr(Documented.captured))
    print(repr(Undocumented.__doc__), repr(Undocumented.captured))
    print(repr(Replaced.__doc__), Replaced().__doc__, isinstance(errno.__doc__, str))

    inherited = (getdoc(Undocumented), getdoc(Undocumented.method),
                 getdoc(Undocumented().method), getdoc(Undocumented.reading))
    method_doc = Documented.method.__doc__
    expected = (getdoc(Documented), method_doc, method_doc, Documented.reading.__doc__)
    if inherited != expected or reads:
        raise AssertionError((inherited, reads))
    if not ismemberdescriptor(Slots.power) or ismemberdescriptor(Undocumented.reading):
        raise AssertionError("incorrect member descriptor classification")
    if (getdoc(Slots.power), getdoc(Slots.unused)) != ("measured in kilowatts", None):
        raise AssertionError("incorrect slot documentation")
