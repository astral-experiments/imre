from collections import abc
import typing
from typing import AsyncIterator, Awaitable, Generic, TypeVar


T = TypeVar("T")
for alias, module_alias, origin in (
    (Awaitable, typing.Awaitable, abc.Awaitable),
    (AsyncIterator, typing.AsyncIterator, abc.AsyncIterator),
):
    assert alias is module_alias
    specialized = alias[T]
    assert specialized is alias[T]
    assert typing.get_origin(alias) is origin
    assert typing.get_origin(specialized) is origin
    assert typing.get_args(specialized) == (T,)

    class Child(specialized):
        pass

    assert Child.__bases__ == (origin, Generic)
    assert Child.__orig_bases__ == (specialized,)
    for parameters, expected in (
        ((), f"Too few arguments for {alias}; actual 0, expected 1"),
        ((int, str), f"Too many arguments for {alias}; actual 2, expected 1"),
    ):
        try:
            alias[parameters]
        except TypeError as error:
            assert str(error) == expected
        else:
            raise AssertionError("expected a single type parameter")
