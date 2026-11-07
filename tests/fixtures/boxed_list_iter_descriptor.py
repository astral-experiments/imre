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
doc = type(slot).__doc__
assert doc is vars(type(slot))["__doc__"]
assert doc.__objclass__ is type(slot) and doc.__name__ == "__doc__"
assert doc.__get__(None, type(slot)) is doc
assert doc.__get__(object.__hash__) == object.__hash__.__doc__ == "Return hash(self)."
try:
    doc.__get__(list.append)
except TypeError as error:
    assert str(error) == ("descriptor '__doc__' for 'wrapper_descriptor' objects "
                          "doesn't apply to a 'method_descriptor' object")
else:
    raise AssertionError("accepted a different kind of descriptor")
try:
    comparison_doc = object.__eq__.__doc__
except AttributeError as missing:
    try:
        doc.__get__(object.__eq__)
    except AttributeError as forwarded:
        assert forwarded.args == missing.args
    else:
        raise AssertionError("lost a slot documentation error")
else:
    assert doc.__get__(object.__eq__) == comparison_doc
for change in (lambda: setattr(slot, "__doc__", "changed"), lambda: delattr(slot, "__doc__")):
    try:
        change()
    except AttributeError as error:
        assert str(error) == "attribute '__doc__' of 'wrapper_descriptor' objects is not writable"
    else:
        raise AssertionError("wrote a read-only descriptor")

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
