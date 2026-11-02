from typing import Annotated, List, NoDefault, Tuple, TypeVar, TypeVarTuple, Unpack, get_args, get_origin


def current():
    ts = TypeVarTuple("Ts")
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

    t = TypeVar("T")
    marker = List[Annotated[t, ts]]
    assert marker.__parameters__ == (t,)
    assert marker[int] == List[Annotated[int, ts]]
    for origin in (tuple, Tuple):
        variadic = Annotated[origin[t, Unpack[ts], t], "metadata"]
        assert variadic[int] == Annotated[origin[int, int], "metadata"]
        assert variadic[int, str, bool] == Annotated[origin[int, str, bool, int], "metadata"]
        alias = variadic[t, *TypeVarTuple("Us")]
        assert alias[bool, float] == Annotated[origin[bool, float, bool], "metadata"]

    for bad in (lambda: isinstance(0, ts), lambda: isinstance(0, NoDefault),
                lambda: Unpack[int, str], lambda: type(NoDefault)(bad=True)):
        try:
            bad()
        except TypeError:
            pass
        else:
            raise AssertionError("invalid operation succeeded")
    print("variadic types")
