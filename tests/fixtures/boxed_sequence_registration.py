from collections import OrderedDict
from collections.abc import Mapping, Sequence
from itertools import count


def pattern_kinds(value):
    match value:
        case [*_]:
            sequence = True
        case _:
            sequence = False
    match value:
        case {}:
            mapping = True
        case _:
            mapping = False
    return sequence, mapping


def current():
    class Parent:
        def __len__(self):
            raise AssertionError("a wildcard pattern requested the length")

        def __iter__(self):
            raise AssertionError("a wildcard pattern consumed the subject")

    class Existing(Parent):
        pass

    assert pattern_kinds(Parent()) == (False, False)
    Sequence.register(Parent)

    class Later(Parent):
        pass

    for cls in (Parent, Existing, Later):
        assert pattern_kinds(cls()) == (True, False)

    Mapping.register(Parent)
    Sequence.register(Parent)  # The existing virtual registration is a no-op.
    for cls in (Parent, Existing, Later):
        assert pattern_kinds(cls()) == (False, True)

    assert pattern_kinds(range(2)) == (True, False)
    for value in ("ab", b"ab", bytearray(b"ab")):
        assert pattern_kinds(value) == (False, False)


    class Counting(count):
        pass

    Sequence.register(count)
    assert pattern_kinds(count()) == (False, False)
    assert pattern_kinds(Counting()) == (False, False)
    Mapping.register(Counting)
    assert pattern_kinds(Counting()) == (False, True)

    class OrderedChild(OrderedDict):
        pass

    Sequence.register(OrderedDict)
    assert pattern_kinds(OrderedDict()) == (False, True)
    assert pattern_kinds(OrderedChild()) == (False, True)
