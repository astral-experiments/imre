from collections import ChainMap, defaultdict
from collections.abc import Collection, Mapping, MutableMapping, MutableSet, Set


class Store(MutableMapping[str, int]):
    def __init__(self):
        self.data = {}

    def __getitem__(self, key):
        return self.data[key]

    def __setitem__(self, key, value):
        self.data[key] = value

    def __delitem__(self, key):
        del self.data[key]

    def __len__(self):
        return len(self.data)

    def __iter__(self):
        return iter(self.data)


class DictChild(dict):
    pass


def hashable_classes():
    from collections.abc import Hashable

    class Inherited(Hashable):
        def __hash__(self):
            return super().__hash__()

    class EqualOnly:
        def __eq__(self, other):
            return self is other

    class RefusesHash:
        def __hash__(self):
            raise ValueError("hash called")

    class Advertised:
        __hash__ = 1

    class Disabled:
        __hash__ = None

    class Enabled(Disabled, RefusesHash):
        __hash__ = RefusesHash.__hash__

    assert not issubclass(Disabled, Hashable)
    assert issubclass(Enabled, Hashable)
    for value in (Inherited(), RefusesHash(), Advertised(), ([],), object(), frozenset(), type):
        assert isinstance(value, Hashable)
        assert issubclass(type(value), Hashable)
    for value in (EqualOnly(), [], {}, set(), bytearray(), {}.keys()):
        assert not isinstance(value, Hashable)
        assert not issubclass(type(value), Hashable)
    assert hash(Inherited()) == 0
    assert not issubclass(int, Inherited)
    try:
        Hashable()
    except TypeError:
        pass
    else:
        raise AssertionError("instantiated abstract Hashable")


def dynamic_collection_classes():
    from abc import ABCMeta
    from collections.abc import Awaitable, Buffer, Coroutine, Sequence

    for base, required in (
        (Awaitable, "__await__"), (Buffer, "__buffer__"), (Coroutine, "throw"),
        (Mapping, "__getitem__"), (Sequence, "__len__"), (Set, "__iter__"),
    ):
        assert type(base) is ABCMeta
        methods = dict.fromkeys(base.__abstractmethods__, lambda self, *args: 0)
        complete = type("Complete", (base,), methods)
        assert type(complete) is ABCMeta and type(complete()) is complete
        del methods[required]
        incomplete = type("Incomplete", (base,), methods)
        assert incomplete.__abstractmethods__ == frozenset({required})
        try:
            incomplete()
        except TypeError as error:
            assert required in str(error)
        else:
            raise AssertionError("instantiated an abstract collection")


def current():
    hashable_classes()
    dynamic_collection_classes()
    from collections.abc import Collection, Container, Iterable, Sized

    class Protocol:
        def __iter__(self):
            return iter(())

        def __len__(self):
            return 0

        def __contains__(self, item):
            return False

    for abc in (Iterable, Sized, Container, Collection):
        assert issubclass(Protocol, abc) and isinstance(Protocol(), abc)
    class NoIter(Protocol):
        __iter__ = None

    assert Iterable.__subclasshook__(NoIter) is NotImplemented
    assert not issubclass(NoIter, Collection)

    from array import array
    from collections import deque
    from collections.abc import Iterator, MutableSequence, Sequence

    class Counter(Iterator[int]):
        def __next__(self):
            return 1

    class Cursor:
        def __iter__(self):
            return self

        def __next__(self):
            raise StopIteration

    counter = Counter()
    assert iter(counter) is counter and next(counter) == 1
    assert isinstance(Cursor(), Iterator) and issubclass(Cursor, Iterator)
    assert isinstance(iter([1]), Iterator) and not isinstance([1], Iterator)
    assert isinstance((item for item in [1]), Iterator)
    with open(__file__, "rb") as stream:
        assert isinstance(stream, Iterator) and issubclass(type(stream), Iterator)

    assert (array.__module__, array.__qualname__, repr(array)) == ("array", "array", "<class 'array.array'>")
    assert issubclass(array, MutableSequence) and issubclass(array, Sequence)
    assert not issubclass(array, MutableMapping) and not issubclass(MutableSequence, array)
    assert isinstance(deque(), MutableSequence) and issubclass(memoryview, Sequence)
    print(MutableMapping.__bases__ == (Mapping,), Store.__bases__ == (MutableMapping,))
    print(Store.__orig_bases__[0].__origin__ is MutableMapping)
    for value in ({}, DictChild(), [("Host", "localhost")], Store()):
        print(isinstance(value, Mapping), isinstance(value, MutableMapping))
    for cls in (DictChild, Store, list):
        print(issubclass(cls, Mapping), issubclass(cls, MutableMapping))
    try:
        MutableMapping()
    except TypeError as error:
        print(str(error))
    data = Store()
    shared = []
    print(data.get("missing", shared) is shared, data.setdefault("first", shared) is shared)
    view = data.keys()
    data.update({"second": 2}, third=3)
    print(len(view), list(view), data.get("second"), data.pop("missing", shared) is shared)
    print(data.pop("second"), list(data.items()))
    data.clear()
    print(len(data), list(view))

    data.update({"a": 1, "b": 2})
    values = iter(data.values())
    print(next(values))
    data["b"] = 3
    print(next(values))

    first, last = defaultdict(list), {"late": [2]}
    chain = ChainMap(first, last)
    saved = chain.maps
    print("late" in chain, chain.get("absent"), first)
    print(chain["late"] is first["late"], last["late"])
    chain["new"] = [3]
    print(list(chain), len(chain), saved[0]["new"])
    chain.maps = []
    print(bool(chain), repr(chain), len(saved))
    chain.maps.append({"owner": chain})
    print(repr(chain))

    class Registered:
        pass

    print(MutableSet.register(Registered) is Registered)
    print(isinstance(Registered(), MutableSet), issubclass(Registered, Collection))
    print(issubclass(Registered, Mapping), MutableSet not in Registered.__mro__)
    for value in ({}, set(), frozenset()):
        print(isinstance(value, Collection), isinstance(value, Set), isinstance(value, MutableSet))
