import types
import typing


def current():
    from typing import Callable, Final, Generic, TypeVar

    parameter = TypeVar("parameter")

    class Container(Generic[parameter]):
        pass

    first, second = [], []
    for left, right in (
        (Final[first], Final[second]),
        (Container[first], Container[second]),
        (Callable[[first], int], Callable[[second], int]),
    ):
        assert left is not right
        assert left.__args__[0] is first
        assert right.__args__[0] is second
    assert Final[int] is Final[int]
    assert Container[int] is Container[int]

    marked, regular = Final[int], Container[int]
    typing.Final = Container
    try:
        try:
            class Invalid(marked):
                pass
        except TypeError as error:
            assert str(error) == "Cannot subclass typing.Final[int]"
        else:
            raise AssertionError("Final alias accepted as a base")

        class Valid(regular):
            pass
        assert issubclass(Valid, Container)
    finally:
        typing.Final = Final
    assert Callable[[int], int] is Callable[[int], int]

    from typing import Final, Literal

    for origin, expected in (
        (Final, "typing.Final[[typing.Final[[...]]]]"),
        (Literal, "typing.Literal[[typing.Literal[[...]]]]"),
    ):
        items = []
        alias = origin[items]
        items.append(alias)
        assert repr(alias) == expected
        items.clear()
        assert typing.get_args(alias)[0] is items
    items = []
    alias = list[items]
    items.append(alias)
    try:
        repr(alias)
    except RecursionError:
        pass
    else:
        raise AssertionError("recursive builtin type argument list")
    items.clear()
    assert repr(alias) == "list[[]]"
    union_args = []
    alias = Literal[union_args]
    union_args.append(alias | int)
    assert repr(alias) == "typing.Literal[[typing.Literal[[...]] | int]]"
    union_args.clear()

    from typing import Final, assert_never

    label: Final = "fixed"
    assert label == "fixed" and Final is typing.Final
    assert repr(Final[int]) == "typing.Final[int]"
    assert typing.get_origin(Final[int]) is Final and typing.get_args(Final[int]) == (int,)

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
