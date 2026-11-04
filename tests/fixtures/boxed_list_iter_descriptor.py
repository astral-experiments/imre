from collections.abc import Mapping
from weakref import ref


class Plain:
    pass


class SequenceList(list, Plain, Mapping):
    pass


class Override(list):
    def __iter__(self):
        return iter(("override",))


assert SequenceList.__abstractmethods__ == frozenset()
slot = list.__iter__
assert slot is list.__iter__ is SequenceList.__iter__
assert slot.__objclass__ is list
assert type(slot).__name__ == "wrapper_descriptor"
assert list(SequenceList([1, 2])) == [1, 2]

source = Override([1, 2])
assert list(source) == ["override"]
cursor = slot(source)
assert type(cursor).__name__ == "list_iterator"
assert next(cursor) == 1
source.append(3)
seen = ref(source)
del source
assert seen() is not None
assert list(cursor) == [2, 3]
assert seen() is None
for args in ((), (1,), ([], 2)):
    try:
        slot(*args)
    except TypeError as error:
        print(str(error))
    else:
        raise AssertionError("invalid list iterator call")


def current():
    items = [1, 2]
    cursor = iter(items)
    advance = cursor.__next__
    descriptor = type(cursor).__next__
    assert descriptor.__objclass__ is type(cursor)
    assert type(advance).__name__ == "method-wrapper"
    assert advance.__self__ is cursor and cursor.__iter__() is cursor
    for wrong in (items, iter((1,)), reversed(items)):
        try:
            descriptor(wrong)
        except TypeError:
            pass
        else:
            raise AssertionError("accepted a different iterator type")
    assert advance() == 1
    assert descriptor(cursor) == 2
    items.append(3)
    assert next(cursor) == 3
    for value in (4, 5):
        try:
            advance()
        except StopIteration:
            items.append(value)
        else:
            raise AssertionError("exhausted list iterator resumed")
