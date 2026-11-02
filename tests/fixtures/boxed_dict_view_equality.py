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
