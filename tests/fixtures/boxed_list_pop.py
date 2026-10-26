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


def current():
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
