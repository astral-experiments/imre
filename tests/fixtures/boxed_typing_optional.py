import types
import typing


def current():
    from typing import assert_never

    assert assert_never is typing.assert_never
    try:
        assert_never("x" * 100)
    except AssertionError as error:
        assert str(error) == "Expected code to be unreachable, but got: '" + "x" * 99 + "..."
    else:
        raise AssertionError("assert_never returned")

    optional = typing.Optional[str]
    print(type(optional) is typing.Union is types.UnionType)
    print(typing.get_args(optional) == (str, type(None)), optional is typing.Optional[str])
    print(optional == (None | str), {optional: "value"}[None | str])
    print(typing.Optional[None] is type(None), isinstance(None, optional))
    print(issubclass(type(None), optional), issubclass(int, optional))
    for argument in ((), (str,), (str, int), typing.Optional):
        try:
            typing.Optional[argument]
        except TypeError as error:
            print(str(error))
    try:
        isinstance(None, typing.Optional[list[int]])
    except TypeError as error:
        print(issubclass(type(error), TypeError), str(error))

    class Unhashable:
        __hash__ = None

    member = Unhashable()
    print(typing.Optional[member] is typing.Optional[member])
