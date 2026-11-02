from itertools import chain


def current():
    calls = []

    class Later:
        def __iter__(self):
            calls.append("opened")
            return iter([2])

    cursor = chain([], [1], Later())
    assert iter(cursor) is cursor and next(cursor) == 1 and not calls
    assert next(cursor) == 2 and calls == ["opened"]
    assert next(cursor, None) is None and next(cursor, None) is None

    cursor = chain([1], None, [2])
    assert next(cursor) == 1
    try:
        next(cursor)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a non-iterable")
    assert next(cursor, None) is None

    class PrivateState(chain):
        def __setattr__(self, name, value):
            raise AssertionError(name)

        def __getattribute__(self, name):
            if name in ("_sources", "_active"):
                raise AssertionError(name)
            return super().__getattribute__(name)

    assert list(PrivateState([1], [2])) == [1, 2]

    assert chain.__init__ is object.__init__
    direct = chain([1, 2])
    assert chain.__init__(direct, [9]) is None and list(direct) == [1, 2]

    class Initialized(chain):
        def __init__(self, *sources, label):
            super().__init__()
            self._sources = label
            self.first = next(self)

    initialized = Initialized([1, 2], label="example")
    assert initialized.first == 1 and list(initialized) == [2]
    assert initialized.__getstate__() == {"_sources": "example", "first": 1}
    try:
        object.__init__(initialized, [9])
    except TypeError:
        pass
    else:
        raise AssertionError("object.__init__ accepted arguments for an overriding initializer")

    class Constructed(chain):
        def __new__(cls, *sources, label):
            item = super().__new__(cls, *sources)
            item.label = label
            return item

    constructed = Constructed([3, 4], label="example")
    assert constructed.label == "example" and list(constructed) == [3, 4]
    class Guard(type):
        def __getattribute__(cls, name):
            if name == "__init__":
                raise AssertionError("consulted the metaclass for the initializer")
            return super().__getattribute__(name)

    class Plain(chain, metaclass=Guard):
        pass

    for subtype in (chain, Constructed, Plain):
        try:
            chain.__new__(subtype, [1], label="unclaimed")
        except TypeError:
            pass
        else:
            raise AssertionError("accepted keywords without a subclass initializer")

    class Mixin:
        pass

    class Compatible(Mixin, chain):
        pass

    assert list(Compatible([1])) == [1]
    try:
        class Incompatible(chain, list):
            pass
    except TypeError:
        pass
    else:
        raise AssertionError("accepted incompatible native layouts")

    class Constructor(chain):
        def __new__(cls, *args):
            raise AssertionError("__new__")

        def __init__(self, *args):
            raise AssertionError("__init__")

        def __setattr__(self, name, value):
            raise AssertionError(name)

    cursor = Constructor.from_iterable([[3], [4]])
    assert type(cursor) is Constructor and list(cursor) == [3, 4]

    import itertools
    for name in ("_cycle_allocate", "_cycle_next", "_filterfalse_allocate", "_filterfalse_state",
                 "_chain_allocate", "_chain_get", "_chain_set", "_chain_next"):
        assert name not in vars(itertools) and not hasattr(itertools, name)
    itertools._chain_get = None
    itertools._chain_set = None
    itertools._chain_allocate = None
    assert list(chain.from_iterable([[7], [8]])) == [7, 8]

    class Impostor:
        pass

    fake = Impostor()
    fake._sources = iter([[1]])
    fake._active = None
    try:
        chain.__next__(fake)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted an unrelated receiver")

    assert error_tails() == ([], [2, 3], [])
    assert reentrant_tails() == ([7, 6, 9], [2, 4])


def error_tails():
    class Steps:
        def __init__(self, values):
            self.values = iter(values)

        def __iter__(self):
            return self

        def __next__(self):
            value = next(self.values)
            if isinstance(value, BaseException):
                raise value
            return value

    failure = ValueError("injected")

    class CannotOpen:
        def __iter__(self):
            raise failure

    tails = []
    for cursor in (chain.from_iterable(Steps([failure, [1]])),
                   chain(Steps([failure, 2]), [3]),
                   chain(CannotOpen(), [4])):
        try:
            next(cursor)
        except ValueError as error:
            assert error is failure
        else:
            raise AssertionError("lost iterator error")
        tails.append(list(cursor))

    class End(StopIteration):
        pass

    cursor = chain.from_iterable(Steps([End("outer payload"), [5]]))
    try:
        next(cursor)
    except StopIteration as stopped:
        assert type(stopped) is StopIteration and not stopped.args
    else:
        raise AssertionError("continued after outer exhaustion")
    assert list(cursor) == []
    assert list(chain(Steps([End("child payload")]), [6])) == [6]
    return tuple(tails)


def reentrant_tails():
    import gc

    opened = []

    class ReenterOnOpen:
        def __iter__(self):
            opened.append(next(cursor))
            gc.collect()
            return iter([6])

    cursor = chain(ReenterOnOpen(), [7, 8], [9])
    opened.extend(list(cursor))

    nested = []

    class ReenterOnNext:
        def __iter__(self):
            return self

        def __next__(self):
            if not nested:
                nested.append(None)
                nested[0] = next(cursor)
                gc.collect()
            raise StopIteration

    cursor = chain(ReenterOnNext(), [2, 3], [4])
    nested.extend(list(cursor))
    return opened, nested
