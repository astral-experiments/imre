class Items:
    def __init__(self, name, values, events):
        self.name = name
        self.values = iter(values)
        self.events = events

    def __iter__(self):
        self.events.append("iter " + self.name)
        return self

    def __next__(self):
        self.events.append("next " + self.name)
        value = next(self.values)
        if value == "fail":
            raise ValueError("source failed")
        if value == "stop":
            raise StopIteration("source stopped")
        return value


def pull(cursor):
    try:
        print(next(cursor))
    except (StopIteration, TypeError, ValueError) as error:
        print(type(error).__name__, str(error))


def current():
    events = []
    class Strict:
        def __bool__(self):
            events.append("strict")
            return True

    def add(left, right):
        events.append("call")
        return left + right

    left = [1, 3]
    cursor = map(add, Items("a", left, events), Items("b", [2, 4, 6], events), strict=Strict())
    print(events, type(cursor) is map, isinstance(cursor, map), iter(cursor) is cursor)
    left.append(5)
    print(next(cursor), list(cursor), next(cursor, "end"), events)
    print(map.__module__, map.__name__, map.__iter__(cursor) is cursor)
    print(map.__next__(map(str, range(1000000000))))

    def callback(value):
        if value == 1:
            raise ValueError("call failed")
        if value == 2:
            raise StopIteration("call stopped")
        return value

    source = Items("x", ["fail", "stop", 1, 2, 3], events)
    cursor = map(callback, source)
    for _ in range(6):
        pull(cursor)
    print(list(map(None, [])))
    cursor = map(None, [1, 2])
    pull(cursor)
    pull(cursor)
    pull(cursor)

    for groups in (([1, 2], [3], [4, 5]), ([1], [2], [3, 4]), ([], ["fail"]), ([1], ["stop"])):
        events = []
        inputs = [Items(str(i), groups[i], events) for i in range(len(groups))]
        cursor = map(lambda *row: row, *inputs, strict=True)
        pull(cursor)
        pull(cursor)
        print(events)

    cursor = map(str, [1, 2, 3])
    for first in cursor:
        print(first, cursor.__next__())
        break
    print(list(cursor))
    for args, kwargs in (((), {}), ((str,), {}), ((str, 1), {}), ((str, []), {"other": True}),
                         ((str, []), {"other": True, "strict": True})):
        try:
            map(*args, **kwargs)
        except TypeError as error:
            print(str(error))
