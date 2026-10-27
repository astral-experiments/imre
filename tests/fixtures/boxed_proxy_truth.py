"""Exercise higher-order truth and attribute operations used by request proxies."""

import functools
from functools import Placeholder, partial


def current():
    class Short:
        def __len__(self):
            return 0

    class Precise(Short):
        def __bool__(self):
            return True

    class Bad:
        def __bool__(self):
            return 1

    class Broken:
        def __bool__(self):
            raise ValueError("truth failed")

    class Index:
        def __index__(self):
            return 4

    class Indexed:
        def __len__(self):
            return Index()

    print(bool(Short()), len(Short()), not Short(), bool(Precise()))
    print(bool(Indexed()), len(Indexed()))
    try:
        bool(Bad())
    except TypeError as error:
        print(str(error))
    try:
        if Broken():
            print("unexpected")
    except ValueError as error:
        print(str(error))

    def combine(*args, **kwargs):
        return args, kwargs

    first = partial(combine, 1, a=2, b=3)
    second = partial(first, 4, b=5)
    print(first.func is combine, second.func is combine, second.args)
    print(second(7, a=8, c=9))
    first.keywords["a"] = 19
    print(first(), second())
    print(partial(getattr, first).func is getattr)
    hole = partial(combine, Placeholder, 2)
    nested_hole = partial(hole, Placeholder, 4)
    print(Placeholder is functools.Placeholder, type(Placeholder).__name__, repr(Placeholder))
    print(hole(1), nested_hole(3))
    for function in (combine, hole):
        try:
            partial(function, Placeholder)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("accepted a trailing Placeholder")
    try:
        hole()
    except TypeError as error:
        print(str(error))
    try:
        partial(combine, value=Placeholder)
    except TypeError as error:
        print(str(error))

    class Bound:
        method = partial(combine, 1)

    owner = Bound()
    args, _ = owner.method(2)
    print(args[0], args[1] is owner, args[2])

    class Kept:
        def __init__(self, tag):
            self.tag = tag

    get_tag = partial(getattr, Kept("bound"), "tag")
    for i in range(2500):
        _ = [i, {}]
    print(get_tag())
