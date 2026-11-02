from functools import cached_property
from types import GenericAlias


def current():
    calls = []

    class Item:
        def compute(self):
            """Return a new value on each call."""
            calls.append(1)
            return len(calls)

        value = cached_property(compute)

    descriptor = Item.value
    assert descriptor is Item.__dict__["value"]
    assert descriptor.attrname == "value" and descriptor.func is Item.compute
    assert descriptor.__doc__ == Item.compute.__doc__
    assert descriptor.__module__ == Item.__module__
    first, second = Item(), Item()
    assert (first.value, first.value, second.value) == (1, 1, 2)
    assert first.__dict__["value"] == 1
    first.value = 9
    assert first.value == 9
    del first.value
    assert first.value == 3

    class Other:
        value = descriptor

    assert Other().value == 4
    try:
        class Invalid:
            renamed = descriptor
    except TypeError as error:
        assert str(error) == (
            "Cannot assign the same cached_property to two different names ('value' and 'renamed')."
        )
    else:
        raise AssertionError("descriptor accepted a different name")

    class Slots:
        __slots__ = ()
        value = descriptor

    try:
        Slots().value
    except TypeError as error:
        assert str(error) == "No '__dict__' attribute on 'Slots' instance to cache 'value' property."
    else:
        raise AssertionError("cached a value without storage")

    missing_name = cached_property(Item.compute)
    try:
        missing_name.__get__(first)
    except TypeError as error:
        assert str(error) == "Cannot use cached_property instance without calling __set_name__ on it."
    else:
        raise AssertionError("used an uninitialized descriptor")

    alias = cached_property[int]
    assert type(alias) is GenericAlias
    assert alias.__origin__ is cached_property and alias.__args__ == (int,)
    print(len(calls))
