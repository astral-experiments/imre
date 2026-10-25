import builtins
import functools
from functools import Placeholder as Hole, partial, singledispatch


def mark(events: list[int], n: int) -> int:
    events.append(n)
    return n


def combine(left: int, right: int) -> int:
    return 10 * left + right


def exercise() -> None:
    ph = functools.Placeholder
    constructor = type(ph)
    print(ph is Hole)
    print(constructor is ph.__class__)
    print(constructor is not ph)
    print(constructor.__name__)
    print(constructor.__qualname__)
    print(constructor.__module__)
    print(repr(constructor))
    print(str(ph))
    print(bool(ph))
    print(callable(ph))
    print(callable(constructor))
    values = [constructor(), ph]
    print(values)
    print(values[0] is values[1])
    print(ph == constructor())
    print(ph != None)
    bound = partial(combine, ph, 4)
    print(bound(9))
    events: list[int] = []
    try:
        constructor(mark(events, 1), keyword=mark(events, 2))
    except TypeError as error:
        print(str(error))
    print(events)
    try:
        ph(mark(events, 3))
    except TypeError as error:
        print(str(error))
    print(events)
    try:
        partial(combine, 1, ph)
    except TypeError as error:
        print(str(error))
    try:
        partial(combine, right=ph)
    except TypeError as error:
        print(str(error))

    events = []
    from_list = partial(combine, values[mark(events, 0)], mark(events, 4))
    print(from_list(8))
    print(from_list(9))
    print(events)
    print(partial(combine, constructor(), 3)(7))
    try:
        partial(combine, mark(events, 1), values[mark(events, 0)])
    except TypeError as error:
        print(str(error))
        print(events)
    try:
        partial(combine, right=values[mark(events, 1)])
    except TypeError as error:
        print(str(error))
        print(events)
    try:
        partial(combine, values[mark(events, 5)], mark(events, 6))
    except IndexError:
        print("lookup failed")
        print(events)
    try:
        partial(ph, mark(events, 7))
    except TypeError as error:
        print(str(error))
        print(events)

    classes = [constructor]
    print(isinstance(ph, constructor))
    print(isinstance(constructor, constructor))
    print(isinstance(constructor, type))
    print(isinstance(ph, type))
    events = []
    print(isinstance(values[mark(events, 1)], classes[mark(events, 0)]))
    print(events)

    @singledispatch
    def ignores(argument):
        return "default"

    events = []
    try:
        partial(builtins.len, values[mark(events, 0)])
    except TypeError as error:
        print(str(error))
    try:
        partial(ignores, values[mark(events, 1)])
    except TypeError as error:
        print(str(error))
    print(events)


exercise()
