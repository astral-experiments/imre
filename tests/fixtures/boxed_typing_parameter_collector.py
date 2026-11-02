import typing
import warnings


def current():
    collect = typing._collect_type_parameters
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert typing._collect_parameters is collect
        assert typing._collect_parameters is collect
    assert len(caught) == 1 and caught[0].category is DeprecationWarning
    assert caught[0].filename == __file__
    del typing._collect_parameters

    T = typing.TypeVar("T")
    P = typing.ParamSpec("P", default=[int])
    Ts = typing.TypeVarTuple("Ts")

    class Foreign:
        __parameters__ = (T,)

    assert collect((T, typing.Callable[P, T], T)) == (T, P)
    assert collect((Foreign,)) == ()
    assert collect(((P, T), (T, P))) == (P, T)
    assert collect((P, T), enforce_default_ordering=False) == (P, T)
    assert typing.List[Foreign()].__parameters__ == (T,)
    assert typing.Generic[T, P].__parameters__ == (T, P)
    assert typing.Generic[typing.Unpack[Ts], T].__parameters__ == (Ts, T)
    assert typing.Callable[P, T].__parameters__ == (P, T)

    class WithParams(typing.Generic[T, P]):
        pass

    assert WithParams.__parameters__ == (T, P)
    callback = typing.Callable[typing.Concatenate[T, P], T]
    assert callback[int, [bytes]] == typing.Callable[[int, bytes], int]
    assert typing.Unpack[Ts][int, str] == (int, str)

    for args, message in (
        ((P, T), "Type parameter ~T without a default follows type parameter with a default"),
        ((typing.Unpack[Ts], P), "Type parameter with a default follows TypeVarTuple"),
    ):
        for operation in (collect, lambda params: typing.Generic[params]):
            try:
                operation(args)
            except TypeError as error:
                assert str(error) == message, str(error)
            else:
                raise AssertionError("accepted invalid parameter order")
    try:
        typing._collect_type_parameters = lambda args, **kwargs: [T]
        replacement = typing.List[Foreign]
        assert replacement.__parameters__ == [T]
        assert list[replacement].__parameters__ == ()
    finally:
        typing._collect_type_parameters = collect
    print("typing parameter collection and specialization")
