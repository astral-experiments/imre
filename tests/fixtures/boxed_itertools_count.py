from itertools import count


class AddOnly:
    def __init__(self, value):
        self.value = value

    def __add__(self, other):
        return Number(self.value + other.value)


class Number(AddOnly):
    def __int__(self):
        raise AssertionError("count must not coerce numeric objects")


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

    for args in ((AddOnly(1),), (0, AddOnly(1))):
        try:
            count(*args)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("count accepted an addition-only object")
    cursor = count(1j, 2j)
    print(next(cursor), next(cursor))
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

    import sys

    calls = []

    class Unit(int):
        def __radd__(self, value):
            calls.append(value)
            return 40

    small = int("42")
    large = int("4096")
    assert next(count(small)) is small
    assert next(count(large)) is not large
    unit = Unit(1)
    cursor = count(sys.maxsize - 1, unit)
    assert repr(cursor) == f"count({sys.maxsize - 1})"
    assert next(cursor) == sys.maxsize - 1 and not calls
    assert next(cursor) == sys.maxsize and calls == [sys.maxsize]
    assert next(cursor) == 40 and calls == [sys.maxsize, 40]

    class Reentrant(Number):
        def __add__(self, other):
            if not calls:
                calls.append("outer")
                assert next(cursor) is self
                return outer_result
            return inner_result

    calls.clear()
    initial, inner_result, outer_result = Reentrant(1), Number(2), Number(3)
    cursor = count(initial, Number(1))
    assert next(cursor) is initial
    assert next(cursor) is outer_result
