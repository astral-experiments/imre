from collections.abc import Callable as ABCCallable
from types import GenericAlias
from typing import Annotated, Callable, ForwardRef, List, NoDefault, ParamSpec, Tuple, TypeVar, TypeVarTuple, Unpack, get_args, get_origin
from weakref import ref


def current():
    ts = TypeVarTuple("Ts")
    assert ref(ts)() is ts
    temporary = ref(TypeVarTuple("Temporary"))
    assert temporary() is None
    assert ts is not TypeVarTuple("Ts")
    assert type(ts) is TypeVarTuple
    assert ts.__module__ == __name__
    assert repr(ts) == ts.__name__ == "Ts"
    assert type(iter(ts)) is type(iter(()))
    name = chr(0xD800)
    assert repr(TypeVarTuple(name)) == name

    assert not ts.has_default() and ts.__default__ is NoDefault
    assert not TypeVarTuple("Ts", default=NoDefault).has_default()
    assert TypeVarTuple("Ts", default=None).has_default()
    assert type(NoDefault)() is NoDefault
    assert type(NoDefault).__module__ == "builtins"
    assert NoDefault.__reduce__() == "NoDefault"

    unpacked, = ts
    assert unpacked is Unpack[ts]
    assert get_origin(unpacked) is Unpack
    assert get_args(unpacked) == unpacked.__parameters__ == (ts,)
    assert repr(unpacked) == "typing.Unpack[Ts]"
    assert unpacked[int, str] == (int, str)
    assert Unpack[tuple[int, str]].__typing_unpacked_tuple_args__ == (int, str)

    p = ParamSpec("P")
    assert Callable[[None, *ts], None].__args__ == (type(None), unpacked, type(None))
    assert Callable[p, None].__args__ == (p, type(None))
    assert Callable[..., None].__args__ == (..., type(None))
    assert Callable[[list[None]], None].__args__[0] == list[None]
    assert ABCCallable[[None], None].__args__ == (None, None)
    assert Callable[..., None] == Callable[..., type(None)]
    assert Callable[..., None] is not Callable[..., type(None)]
    assert Callable[[None], None] is Callable[(None,), None]
    assert Callable[["Input"], "Output"].__args__ == (ForwardRef("Input"), ForwardRef("Output"))

    alias = tuple[*ts]
    it = iter(alias)
    starred = next(it)
    assert iter(it) is it and next(it, None) is None
    assert type(starred) is GenericAlias and get_origin(starred) is tuple
    assert get_args(starred) is alias.__args__ and starred.__parameters__ == (ts,)
    assert not alias.__unpacked__ and starred.__unpacked__
    assert alias.__typing_unpacked_tuple_args__ is None
    assert starred.__typing_unpacked_tuple_args__ is alias.__args__
    assert repr(starred) == "*" + repr(alias)
    again, = starred
    assert again is not starred and again == starred and starred != alias
    assert hash(starred) == hash(alias) and len({starred, alias, again}) == 2
    t = TypeVar("T")
    spelled, = list[t]
    assert spelled.__typing_unpacked_tuple_args__ is None
    assert spelled[int] == next(iter(list[int]))
    assert list[t][int] == list[int]
    assert tuple[spelled][str].__args__ == (next(iter(list[str])),)
    assert starred[int, str] == next(iter(tuple[int, str]))
    assert starred["Target", None].__args__ == ("Target", None)

    marker = List[Annotated[t, ts]]
    assert marker.__parameters__ == (t,)
    assert marker[int] == List[Annotated[int, ts]]
    for origin in (tuple, Tuple):
        variadic = Annotated[origin[t, Unpack[ts], t], "metadata"]
        assert variadic[int] == Annotated[origin[int, int], "metadata"]
        assert variadic[int, str, bool] == Annotated[origin[int, str, bool, int], "metadata"]
        alias = variadic[t, *TypeVarTuple("Us")]
        assert alias[bool, float] == Annotated[origin[bool, float, bool], "metadata"]

    spread = Unpack[tuple["Target", None]]
    builtin = Annotated[tuple[*ts], 1]
    named = Annotated[Tuple[*ts], 1]
    assert builtin["Target", None].__origin__.__args__ == (ForwardRef("Target"), type(None))
    assert builtin[spread].__origin__.__args__ == ("Target", None)
    assert named[spread].__origin__.__args__ == (ForwardRef("Target"), type(None))
    try:
        builtin[Unpack[tuple[None, ...]], Unpack[tuple[int, ...]]]
    except TypeError as exc:
        assert str(exc) == "More than one unpacked arbitrary-length tuple argument"
    else:
        raise AssertionError("accepted two arbitrary-length tuple arguments")

    for bad in (lambda: isinstance(0, ts), lambda: isinstance(0, NoDefault),
                lambda: Unpack[int, str], lambda: type(NoDefault)(bad=True)):
        try:
            bad()
        except TypeError:
            pass
        else:
            raise AssertionError("invalid operation succeeded")
    print("variadic types")
