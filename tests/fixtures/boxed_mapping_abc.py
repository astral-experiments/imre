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


def callable_classes():
    from abc import ABCMeta
    from typing import Callable as TypingCallable, get_origin
    from collections.abc import Callable

    assert get_origin(TypingCallable) is Callable
    assert type(Callable) is ABCMeta and Callable.__abstractmethods__ == frozenset({"__call__"})

    class Incomplete(Callable):
        pass

    class Concrete(Callable):
        def __call__(self, *args, **kwargs):
            return super().__call__(*args, **kwargs)

    class Masked(Concrete):
        __call__ = None

    class Shadowed:
        __call__ = None

    class InstanceOnly:
        pass

    value = InstanceOnly()
    value.__call__ = lambda: True
    for item in (value, Shadowed(), [], None):
        assert Callable.__subclasshook__(type(item)) is NotImplemented
        assert not isinstance(item, Callable) and not issubclass(type(item), TypingCallable)
    assert callable(Shadowed()) and not isinstance(Shadowed(), TypingCallable)
    from operator import attrgetter
    from struct import Struct
    for abc in (Callable, TypingCallable):
        assert isinstance(attrgetter("name"), abc)
        assert not isinstance(Struct("B"), abc)

    class Reported:
        @property
        def __class__(self):
            raise NotImplementedError("reported class")

    try:
        isinstance(Reported(), Callable)
    except NotImplementedError as exc:
        assert str(exc) == "reported class"
    else:
        raise AssertionError("suppressed the reported class error")
    assert Concrete()(4, key=3) is False
    assert get_origin(Callable[[int], bool]) is Callable
    assert get_origin(TypingCallable[..., int]) is Callable
    for item in (lambda: 1, len, list.append, [].append, Concrete().__call__, type, staticmethod(len)):
        assert isinstance(item, Callable) and issubclass(type(item), Callable)
    assert issubclass(Masked, Callable) and isinstance(Masked(), Callable)
    from typing import List, Type
    assert issubclass(TypingCallable, TypingCallable) and issubclass(Type, TypingCallable)
    assert not issubclass(List, TypingCallable)
    try:
        issubclass(TypingCallable[..., int], TypingCallable)
    except TypeError as exc:
        assert str(exc) == "Subscripted generics cannot be used with class and instance checks"
    else:
        raise AssertionError("accepted a parameterized typing alias")

    assert Callable.register(InstanceOnly) is InstanceOnly
    assert isinstance(value, TypingCallable) and issubclass(InstanceOnly, Callable)
    try:
        Incomplete()
    except TypeError as exc:
        assert "__call__" in str(exc)
    else:
        raise AssertionError("instantiated abstract Callable subclass")


def current():
    callable_classes()
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

    class Local(ChainMap):
        pass

    parent = {"inherited": 1}
    first = {}
    child = Local(parent).new_child(first, current=2)
    assert type(child) is Local and child.maps[0] is first
    assert child.parents.maps[0] is parent and first == {"current": 2}
    clone = child.copy()
    assert clone.maps[0] is not first and clone.maps[1] is parent
    assert clone.maps == child.maps and Local.__copy__ is Local.copy
    assert child.pop("inherited", None) is None
    try:
        child.pop("inherited")
    except KeyError as exc:
        assert exc.args == ("Key not found in the first mapping: 'inherited'",)
    else:
        raise AssertionError("popped a parent key")
    assert child.popitem() == ("current", 2)
    original = child
    child |= [("local", 3)]
    assert child is original and first == {"local": 3}
    merged = child | {"local": 4}
    assert type(merged) is Local and merged.maps == [{"local": 4}, parent]
    assert merged.maps[1] is parent and merged.maps[0] is not first
    assert ({"inherited": 0} | child).maps == [{"inherited": 1, "local": 3}]
    child.clear()
    assert first == {} and parent == {"inherited": 1}

    class Registered:
        pass

    print(MutableSet.register(Registered) is Registered)
    print(isinstance(Registered(), MutableSet), issubclass(Registered, Collection))
    print(issubclass(Registered, Mapping), MutableSet not in Registered.__mro__)
    for value in ({}, set(), frozenset()):
        print(isinstance(value, Collection), isinstance(value, Set), isinstance(value, MutableSet))
