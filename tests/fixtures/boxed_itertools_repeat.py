from itertools import repeat
from operator import length_hint


class Index:
    def __index__(self):
        print("index once")
        return 2


class BadIndex:
    def __index__(self):
        raise ValueError("index failed")


def current():
    value = []
    cursor = repeat(object=value, times=Index())
    alias = iter(cursor)
    print(type(cursor) is repeat, isinstance(cursor, repeat), alias is cursor)
    print(repeat.__module__, repeat.__qualname__, repr(cursor), cursor.__length_hint__())
    first = next(alias)
    first.append("changed")
    print(first is value, repr(cursor), cursor.__length_hint__())
    print(cursor.__next__() is value, repr(alias), alias.__length_hint__())
    for _ in range(2):
        try:
            next(cursor)
        except StopIteration:
            print("exhausted", next(alias, "end"))

    forever = repeat(value)
    for item in forever:
        print(item is value, next(forever) is item, repr(forever))
        break
    try:
        forever.__length_hint__()
    except TypeError as error:
        print(str(error))

    for n in (0, -1, -42, False, True, 3):
        bounded = repeat(None, n)
        print(repr(bounded), length_hint(bounded, 92), list(bounded), list(bounded), repr(bounded))
    print(length_hint(forever, 92))
    keys = ("first", "second", "first")
    entries = dict(zip(keys, repeat(value)))
    value.append("later")
    print(list(entries), entries["first"] is entries["second"], entries["first"] is value)
    print(entries["second"], dict(zip((), repeat(value))))
    retained = repeat([42], 1)
    for index in range(1600):
        unused = [index]
    print(next(retained), list(retained))
    maximum = repeat("x", (1 << 63) - 1)
    print(maximum.__length_hint__(), next(maximum), maximum.__length_hint__())

    for args, kwargs in [((), {}), ((), {"other": 1}), ((), {"times": 1}),
                         ((1, 2, 3), {}), ((1,), {"object": 1}), ((1,), {"n": 1}),
                         ((1, None), {}), ((1, 2.5), {}), ((1, 1 << 80), {}),
                         ((1, -(1 << 80)), {}), ((1, BadIndex()), {})]:
        try:
            repeat(*args, **kwargs)
        except (TypeError, ValueError, OverflowError) as error:
            print(type(error).__name__, str(error))
