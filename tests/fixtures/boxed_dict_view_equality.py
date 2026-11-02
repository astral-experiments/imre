import typing

events = []

class Hooked(set):
    def __len__(self):
        events.append('len')
        return 1
    def __contains__(self, item):
        events.append('contains')
        return item == 1
    def __eq__(self, other):
        events.append('eq')
        return NotImplemented

view = {1: 'a'}.keys()
assert view == {1} and frozenset((1,)) == view, 'matching dictionary keys'
assert view != {2} and {2} != view
assert view != [1] and [1] != view
assert (view == Hooked()) is True
assert events == ['len', 'contains'], events
events.clear()
assert (Hooked() == view) is True
assert events == ['eq', 'len', 'contains'], events
events.clear()
assert (Hooked() != view) is False
assert events == ['len', 'contains'], events
print('reflected inequality', events)

class EqualityOnly(frozenset):
    def __eq__(self, other):
        raise AssertionError('inherited set inequality must decline before object.__ne__')

assert (EqualityOnly((1,)) != view) is False
assert [view] == [frozenset((1,))]
items = {1: [2]}.items()
assert items == {1: [2]}.items() and items != {1: [3]}.items()
assert items != set()
try:
    items == {(2, 3)}
except TypeError as e:
    print(type(e).__name__, e)
else:
    raise AssertionError('unhashable item should raise on a set membership probe')
assert {('x', 1): 3}.keys() == {'x': 1}.items()
assert {1: 3}.values() != {3}
data = {1: 3}
live = data.keys()
data[2] = 4
assert live == {1, 2}
print('dictionary view equality')

class OrderedSet(set):
    def __iter__(self):
        events.append('iter')
        return iter((1,))
    def __len__(self):
        events.append('len')
        return 1
    def __contains__(self, key):
        events.append('contains')
        return key == 1
    def __le__(self, other):
        events.append('le')
        return NotImplemented
    def __ge__(self, other):
        return 'handled'

events.clear()
assert view <= OrderedSet() and events == ['len', 'contains']
events.clear()
assert OrderedSet() <= view and events == ['le', 'len', 'iter']
assert (OrderedSet() >= view) == 'handled'
assert view < frozenset((1, 2)) and {1, 2} > view
assert not view < {1} and not {1} > view
assert {1, 2} >= view and not {2} >= view
assert items >= {1: [2]}.items() and not items <= {1: [3]}.items()
assert {(1, 2): None}.keys() <= {1: 2}.items()
assert not items < {(2, 3)}  # Length decides before probing the unhashable item.
try:
    items <= {(2, 3)}
except TypeError:
    pass
else:
    raise AssertionError('set membership must hash the item')
for left, right in ((view, [1]), ({1: 3}.values(), {3}), (frozenset(), {1: 3}.values())):
    try:
        left <= right
    except TypeError:
        pass
    else:
        raise AssertionError('expected unsupported operands')
print('dictionary view ordering')

class GrowingKey:
    armed = False
    def __hash__(self):
        if self.armed:
            values.add(0)
        return 101

key = GrowingKey()
values = {key}
target = {key: 2}.keys()
key.armed = True
try:
    target >= values
except RuntimeError as exc:
    assert str(exc) == 'Set changed size during iteration'
else:
    raise AssertionError('the live set iterator should detect its mutation')


def current():
    import gc
    import weakref
    from collections import OrderedDict
    from collections.abc import KeysView
    from types import MappingProxyType

    class HookedDict(dict):
        def __getitem__(self, key):
            return ('hook', super().__getitem__(key))

    def rejects(error, operation, *args):
        try:
            operation(*args)
        except error:
            return
        raise AssertionError(operation)

    assert not hasattr(KeysView({}), 'mapping')
    for make in (dict, HookedDict, OrderedDict):
        for name in ('keys', 'values', 'items'):
            owner = make()
            view = getattr(owner, name)()
            base = type(getattr({}, name)())
            descriptor = base.__dict__['mapping']
            assert type(view).mapping is descriptor and descriptor.__objclass__ is base
            assert descriptor.__get__(None, base) is descriptor
            proxy = view.mapping
            assert type(proxy) is MappingProxyType and proxy is not view.mapping
            assert len(proxy) == 0
            owner['x'] = 1
            assert proxy['x'] == owner['x']
            assert descriptor.__get__(view)['x'] == owner['x']
            del owner['x']
            assert 'x' not in proxy
            rejects(AttributeError, setattr, view, 'mapping', None)
            rejects(AttributeError, delattr, view, 'mapping')
            rejects(AttributeError, descriptor.__set__, view, None)
            rejects(AttributeError, descriptor.__delete__, view)
            wrong_kind = {}.items() if name == 'keys' else {}.keys()
            rejects(TypeError, descriptor.__get__, wrong_kind)
            rejects(TypeError, descriptor.__set__, wrong_kind, None)
            rejects(TypeError, descriptor.__delete__, wrong_kind)
    owner = HookedDict(x=3)
    ref = weakref.ref(owner)
    view = owner.keys()
    proxy = view.mapping
    del view, owner
    gc.collect()
    assert ref() is not None and proxy['x'] == ('hook', 3)
    try:
        proxy['x'] = 4
    except TypeError:
        pass
    else:
        raise AssertionError('mappingproxy accepted item assignment')
    try:
        del proxy['x']
    except TypeError:
        pass
    else:
        raise AssertionError('mappingproxy accepted item deletion')
    assert ref()['x'] == ('hook', 3)
    del proxy
    gc.collect()
    assert ref() is None
    print('dictionary view mapping')
