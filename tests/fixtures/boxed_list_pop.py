class Index:
    def __init__(self, items, value):
        self.items = items
        self.value = value

    def __index__(self):
        self.items.append('from __index__')
        return self.value


class DrainingIndex:
    def __init__(self, items):
        self.items = items

    def __index__(self):
        self.items.pop()
        return 0


def growth_allocation(name, bound):
    class Values(list):
        pass

    values = Values()
    first = object()
    last = object()
    for _ in range(128):
        list.append(values, first)
    grow = getattr(values if bound else list, name)
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        if name == "append":
            if bound:
                grow(last)
            else:
                grow(values, last)
        elif bound:
            grow(128, last)
        else:
            grow(values, 128, last)
    except MemoryError as error:
        print("caught", error.args)
    print(len(values), values[0] is first, values[-1] is last)
    if name == "append":
        values.append(last)
    else:
        values.insert(128, last)
    print("retry", len(values), values[-1] is last)


def current():
    class Source(list):
        def __iter__(self):
            return iter((9,))

    items = Source([1, 2])
    items.extend(items)
    items.extend(Source())
    assert items == [1, 2, 1, 2, 9]

    class Values(list):
        def append(self, item):
            return "override"

    values = Values()
    assert list.append(values, 3) is None
    assert values == [3] and values.append(4) == "override"
    add = list.append.__get__(values, Values)
    take = list.pop.__get__(values, Values)
    assert add(7) is None and take() == 7

    for name in ('clear', 'copy', 'count', 'extend', 'index', 'insert', 'remove', 'reverse', 'sort'):
        method = getattr(Values, name)
        assert method is getattr(list, name) and method.__objclass__ is list
        assert method.__get__(values, Values).__self__ is values

    class ClearOnMatch:
        def __eq__(self, other):
            values.clear()
            return True

    for method in (list.count, list.remove):
        values.clear()
        values.extend((ClearOnMatch(), 1))
        result = method(values, object())
        assert result == (1 if method is list.count else None) and not values

    class Wrong:
        push = list.append

    try:
        Wrong().push
    except TypeError:
        pass
    else:
        raise AssertionError("list.append bound an unrelated receiver")

    seen = []
    try:
        Wrong().push(seen.append(1))
    except TypeError:
        pass
    assert seen == [1]

    from copy import copy

    element = []
    original = [element, element]
    for clone in (original.copy(), list.copy(original), copy(original)):
        assert clone is not original and clone[0] is clone[1] is element
        clone.pop()
    assert len(original) == 2
    retained = {}
    items = [retained, 2, 3]
    alias = items
    print(items.pop(), list.pop(items, True), items.pop(-1) is retained, alias)

    for original in ([], [1, 2]):
        for args, kwargs in (((), {}), ((-3,), {}), ((2,), {}), ((None,), {}),
                             ((1 << 80,), {}), ((0, 1), {}), ((), {'index': 0})):
            items = list(original)
            try:
                print(items.pop(*args, **kwargs), items)
            except (TypeError, IndexError, OverflowError) as error:
                print(str(error), items)

    items = [1, 2]
    items.insert(Index(items, -1), 3)
    assert items == [1, 2, 3, 'from __index__']

    items = []
    print(items.pop(Index(items, 0)), items)
    try:
        items.pop(Index(items, 'invalid'))
    except TypeError as error:
        print(str(error), items)
    try:
        items.pop(DrainingIndex(items))
    except IndexError as error:
        print(str(error), items)
    items = [0, 1, 2, 3]
    for item in items:
        print(item)
        if item == 0:
            items.pop(0)
    for args in ((), (None,)):
        try:
            list.pop(*args)
        except TypeError as error:
            print(str(error))


def extend_allocation(kind):
    old, new = object(), object()
    items = [old] * 128
    source = items if kind == 'self' else [new] * 117
    if kind == 'tuple':
        source = tuple(source)
    last = source[-1]
    extend = items.extend
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        extend(source)
    except MemoryError as error:
        print("caught", error.args)
        assert len(items) == 128 and all(value is old for value in items)
    expected = len(items) + len(source)
    extend(source)
    print(len(items) == expected, items[0] is old, items[-1] is last)
