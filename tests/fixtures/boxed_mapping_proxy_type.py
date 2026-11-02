from dataclasses import field
from types import MappingProxyType


def current():
    values = {'a': 1}
    view = MappingProxyType(mapping=values)
    for name in ("keys", "values", "items", "get", "copy"):
        method = getattr(view, name)
        assert method.__self__ is view
        assert getattr(MappingProxyType, name).__get__(view).__self__ is view
        try:
            getattr(MappingProxyType, name)(values)
        except TypeError:
            pass
        else:
            raise AssertionError("accepted the backing dict as a proxy receiver")
    try:
        hash(view)
    except TypeError as exc:
        assert str(exc) == "unhashable type: 'dict'"
    else:
        raise AssertionError("hashed a proxy to an unhashable dictionary")

    class HashableDict(dict):
        def __hash__(self):
            return self["value"]

    backing = HashableDict(value=37)
    nested = MappingProxyType(MappingProxyType(backing))
    assert hash(nested) == 37
    backing["value"] = 42
    assert nested.__hash__() == MappingProxyType.__hash__(nested) == 42
    assert {nested: "proxied"}[backing] == "proxied"

    getter = MappingProxyType.__getitem__
    assert getter.__objclass__ is MappingProxyType and type(getter).__name__ == "wrapper_descriptor"
    assert getter.__doc__ == "Return self[key]." and getter.__text_signature__ == "($self, key, /)"
    assert type(view.__getitem__).__name__ == "method-wrapper" and view.__getitem__.__self__ is view
    assert getter(view, "a") == view.__getitem__("a") == 1
    try:
        getter(values, "a")
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a dict as a proxy descriptor receiver")
    assert view.get("a") == 1
    copied = view.copy()
    copied.clear()
    assert view["a"] == 1

    class Lookup(dict):
        def get(self, *args):
            return args

    custom = MappingProxyType(Lookup())
    assert custom.get("a") == ("a", None)
    assert custom.get("a", 2) == ("a", 2)
    assert type(view) is type(field(metadata=values).metadata) is MappingProxyType
    assert MappingProxyType.__module__ == 'builtins'
    assert MappingProxyType.__name__ == 'mappingproxy'
    assert MappingProxyType.__repr__ is not dict.__repr__
    assert MappingProxyType.__repr__(view) == view.__repr__() == "mappingproxy({'a': 1})"
    assert MappingProxyType.__str__(view) == view.__str__() == "{'a': 1}"
    class Display(str):
        pass

    display = Display('mapping display')

    class Mapping(dict):
        def __str__(self):
            return display

    assert MappingProxyType.__str__(MappingProxyType(Mapping())) is display
    values['b'] = 2
    assert list(view) == ['a', 'b'] and view['b'] == 2
    assert list(MappingProxyType(view)) == ['a', 'b']

    try:
        view['c'] = 3
    except TypeError:
        pass
    else:
        raise AssertionError("writable mapping proxy")

    for invalid in ([], (1,), 42):
        try:
            MappingProxyType(invalid)
        except TypeError:
            pass
        else:
            raise AssertionError("accepted a non-mapping")

    try:
        MappingProxyType.__repr__(values)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a dict as a proxy descriptor receiver")
    assert view | {'a': 3} == {'a': 3, 'b': 2}
    assert {'a': 3} | view == values
    assert values == {'a': 1, 'b': 2}

    class Left:
        def __or__(self, other):
            return 'left' if type(other) is MappingProxyType else NotImplemented

    class UnionMapping(dict):
        def __or__(self, other):
            return 'left backing', other

        def __ror__(self, other):
            return 'right backing', other

    mapping = UnionMapping()
    proxy = MappingProxyType(mapping)
    assert Left() | proxy == 'left'
    assert proxy | view == ('left backing', values)
    assert {} | proxy == ('right backing', {})
    side, received = proxy | MappingProxyType(view)
    assert side == 'left backing' and received is view
    assert values.keys() | proxy == {'a', 'b'}
    seen = []

    class Fallback:
        def __or__(self, other):
            seen.append(other)
            return NotImplemented

    fallback = Fallback()
    assert fallback | MappingProxyType(proxy) == ('right backing', fallback)
    assert len(seen) == 3
    assert type(seen[0]) is MappingProxyType and seen[1] is proxy and seen[2] is mapping
    import typing
    import weakref

    assert (typing.Literal[1] | proxy).__args__[-1] is proxy
    assert list[int] | proxy == ('right backing', list[int])

    class ReturnsOperand(dict):
        def __or__(self, other):
            return other

    class Target:
        pass

    target = Target()
    reference = weakref.proxy(target)
    returning = MappingProxyType(ReturnsOperand())
    assert returning | reference is reference
    del target
    assert returning | reference is reference

    class Child(UnionMapping):
        def __ror__(self, other):
            return 'subclass first'

    assert proxy | MappingProxyType(Child()) == 'subclass first'
    return 0
