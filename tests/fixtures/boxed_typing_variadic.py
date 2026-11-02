import sys
from annotationlib import Format, get_annotations
from collections.abc import Callable as ABCCallable
from types import GenericAlias, ModuleType
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

    def vararg_annotation(items):
        def variadic(*args: *items):
            pass
        return variadic.__annotations__["args"]

    assert vararg_annotation(ts) is unpacked

    def unresolved(*args: *Missing["field"]):
        pass

    assert get_annotations(unresolved, format=Format.STRING) == {"args": "*Missing['field']"}
    forward = get_annotations(unresolved, format=Format.FORWARDREF)["args"]
    assert type(forward) is ForwardRef and forward.__owner__ is unresolved
    assert forward.__forward_arg__ == "*Missing['field']"

    try:
        vararg_annotation((int, str))
    except ValueError as exc:
        assert str(exc) == "too many values to unpack (expected 1, got 2)"
    else:
        raise AssertionError("accepted two items in a starred annotation")

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

    previous = sys.modules["typing"]
    sys.modules["typing"] = ModuleType("replacement")
    try:
        assert Callable[p, None].__args__ == (p, type(None))
        assert Callable[[bytes], None].__args__ == (bytes, type(None))
        unhashable = {}
        assert Callable[[unhashable], None].__args__[0] is unhashable
    finally:
        sys.modules["typing"] = previous

    class Parameters(list):
        def __iter__(self):
            return iter((None, int))

    assert Callable[Parameters([bytes]), None] is Callable[[None, int], None]
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

    from typing import Generic
    class Pack(Generic[*ts]): pass

    import typing
    from types import GeneratorType
    from typing import Concatenate, Literal
    from collections.abc import Iterable

    for alias in (Pack[int, str], List[int], Literal[1], Concatenate[int, p], unpacked):
        assert isinstance(alias, Iterable)
        it = alias.__iter__()
        assert type(it) is GeneratorType and iter(it) is it
        assert next(it) is Unpack[alias] and next(it, None) is None
        assert (*alias,) == (Unpack[alias],)
    method = type(Pack[int]).__iter__
    assert method.__name__ == method.__code__.co_name == "__iter__"
    assert method.__qualname__ == method.__code__.co_qualname == "_GenericAlias.__iter__"
    assert iter(Pack[int]).gi_code is method.__code__
    assert vararg_annotation(Pack[int, str]) is Unpack[Pack[int, str]]
    assert Callable[[int], str].__iter__ is None and not isinstance(Callable[[int], str], Iterable)
    it = iter(Pack[int, str])
    try:
        retained = Pack[int, str]
        sys.modules["typing"] = ModuleType("replacement")
        typing.Unpack = {retained: "late lookup"}
        assert next(it) == next(iter(retained)) == "late lookup"
    finally:
        typing.Unpack = Unpack
        sys.modules["typing"] = typing

    u = TypeVar("U")
    class Mixed(Generic[t, *ts, u]): pass
    class Fixed(Generic[t, u]): pass
    class Signature(Generic[p, t]): pass
    class Parameters(Generic[p]): pass

    spelling = [int, u]
    signature = Signature[spelling, Callable[[u], t]]
    spelling[0] = bytes
    assert signature.__args__[0] == (int, u)
    assert signature.__parameters__ == (u, t)
    assert signature[str, bool] == Signature[[int, str], Callable[[str], bool]]
    assert Parameters[int, str] == Parameters[[int, str]]
    assert Parameters[(int, str),] is Parameters[(int, str),]
    assert Parameters[[int, str]] is not Parameters[[int, str]]
    try:
        Signature[[int]]
    except TypeError:
        pass
    else:
        raise AssertionError("missing return parameter accepted")

    defaulted = TypeVarTuple("Defaulted", default=Unpack[tuple[str, int]])
    class Defaulted(Generic[*defaulted]): pass
    other = TypeVarTuple("Other")
    class TwoPacks(Generic[*ts, *other]): pass

    assert Pack[()].__args__ == ()
    assert Pack[int] is Pack[int]
    assert Pack[int] == Pack[(int,)] and Pack[int] is not Pack[(int,)]
    assert Pack[None] == Pack[type(None)] and Pack[None] is not Pack[type(None)]
    value = []
    assert Pack[value].__args__[0] is value and Pack[value] is not Pack[value]
    assert Mixed[int, str].__args__ == (int, str)
    assert Mixed[int, bytes, float, str].__args__ == (int, bytes, float, str)
    assert Mixed[*tuple[int, ...]].__args__ == (int, *tuple[int, ...], int)
    assert Defaulted[()].__args__ == (str, int) and Defaulted[bool].__args__ == (bool,)
    for invalid in (lambda: Mixed[int], lambda: Fixed[int], lambda: Fixed[int, str, bytes],
                    lambda: TwoPacks[int], lambda: Mixed[*tuple[int, ...], *tuple[str, ...]]):
        try:
            invalid()
        except TypeError:
            pass
        else:
            raise AssertionError("invalid generic specialization succeeded")

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
