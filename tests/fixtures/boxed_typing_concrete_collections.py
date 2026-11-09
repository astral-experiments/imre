import collections
import typing
from typing import ChainMap, Deque, OrderedDict, TypeVar, get_args, get_origin


def current():
    parameter = TypeVar("parameter")
    aliases = (
        (Deque, collections.deque, (int,)),
        (ChainMap, collections.ChainMap, (str, int)),
        (OrderedDict, collections.OrderedDict, (str, int)),
    )
    for alias, origin, args in aliases:
        name = alias.__name__
        assert getattr(typing, name) is alias
        assert repr(alias) == "typing." + name and alias.__module__ == "typing"
        assert alias.__doc__ == "Deprecated alias to collections." + origin.__name__ + "."
        assert get_origin(alias) is origin and get_args(alias) == ()
        specialized = alias[args]
        assert get_origin(specialized) is origin and get_args(specialized) == args
        substituted = alias[(parameter,) + args[1:]][args[0]]
        assert substituted == specialized and get_args(substituted) == args
        assert type(alias()) is origin and type(specialized()) is origin
        assert isinstance(origin(), alias) and issubclass(origin, alias)
        assert not isinstance(42, alias) and not issubclass(int, alias)
        try:
            alias[args + (bytes,)]
        except TypeError as exc:
            assert str(exc) == f"Too many arguments for {alias}; actual {len(args) + 1}, expected {len(args)}"
        else:
            raise AssertionError("accepted too many arguments")
        try:
            alias[()]
        except TypeError as exc:
            assert str(exc) == f"Too few arguments for {alias}; actual 0, expected {len(args)}"
        else:
            raise AssertionError("accepted zero arguments")
        try:
            isinstance(origin(), specialized)
        except TypeError as exc:
            assert str(exc) == "Subscripted generics cannot be used with class and instance checks"
        else:
            raise AssertionError("accepted a parameterized instance check")

    class Bins(collections.defaultdict):
        pass

    raw = vars(collections.defaultdict)["__class_getitem__"]
    assert raw.__objclass__ is collections.defaultdict
    assert raw.__get__(None, Bins).__self__ is Bins
    for cls in (collections.defaultdict, Bins):
        alias = cls[str, int]
        assert get_origin(alias) is cls and get_args(alias) == (str, int)
        assert cls().__class_getitem__.__self__ is cls
        assert raw(cls, (str, int)) == alias
        counts = alias(int)
        assert type(counts) is cls and counts["missing"] == 0

    queue = Deque[int]([2, 1], maxlen=2)
    queue.append(3)
    assert list(queue) == [1, 3] and queue.maxlen == 2
    left = {"shared": 1, "left": 2}
    right = {"right": 3, "shared": 4}
    chain = ChainMap[str, int](left, right)
    assert chain.maps[0] is left and chain.maps[1] is right
    assert chain["shared"] == 1 and list(chain) == ["right", "shared", "left"]
    ordered = OrderedDict[str, int]([("b", 2), ("a", 1)])
    ordered.move_to_end("b")
    assert list(ordered.items()) == [("a", 1), ("b", 2)]
    print("concrete collection aliases")
