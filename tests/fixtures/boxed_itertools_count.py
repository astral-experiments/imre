from itertools import count


class Number:
    def __init__(self, value):
        self.value = value

    def __add__(self, other):
        return Number(self.value + other.value)


def current():
    counter = count()
    alias = iter(counter)
    print(type(counter) is count, isinstance(counter, count), alias is counter.__iter__())
    print(count.__module__, count.__name__, repr(counter), next(alias))
    for value in counter:
        print(value, alias.__next__(), repr(counter))
        break
    print(list(zip(("a", "b"), counter)), next(alias))

    huge = 1 << 120
    for start, step in ((huge, -huge), (-2, 0), (2, 0.5), (2.5, 1), (2.5, 1.0),
                        (True, False), (False, True)):
        cursor = count(step=step, start=start)
        print(repr(cursor), cursor.__next__(), cursor.__next__(), repr(cursor))
    start = 1 << 300
    cursor = count(start, 2)
    print(cursor.__next__() is start)

    cursor = count(Number(1), Number(2))
    print(next(cursor).value, next(cursor).value)

    for start, step in ((0.5, 1 << 1100), (1 << 1100, 0.5)):
        cursor = count(start, step)
        before = repr(cursor)
        for _ in range(2):
            try:
                cursor.__next__()
            except OverflowError as error:
                print(str(error), repr(cursor) == before)
    for args, kwargs in [(("x",), {}), ((0, None), {}), ((0, 1, 2), {}),
                         ((1,), {"start": 2}), ((), {"other": 1})]:
        try:
            count(*args, **kwargs)
        except TypeError as error:
            print(str(error))
