from collections.abc import Callable, Coroutine, Iterable
from types import GenericAlias
from typing import Callable as TypingCallable, Concatenate, ParamSpec


def current():
    alias = Callable[[int], str]
    cls = type(alias)
    assert isinstance(alias, GenericAlias)
    assert cls.__mro__ == (cls, GenericAlias, object)
    assert cls.__base__ is GenericAlias and cls.__bases__ == (GenericAlias,)
    assert not isinstance(TypingCallable[[int], str], GenericAlias)
    assert type(Iterable[int]) is type(Coroutine[int, int, str]) is GenericAlias

    p = ParamSpec("P")
    for parameters in (p, Concatenate[int, p], ...):
        direct = cls(origin=Callable, args=(parameters, str))
        assert type(direct) is cls and direct.__origin__ is Callable
        assert direct.__args__ == (parameters, str)
        assert direct == Callable[parameters, str]

    direct = cls(list, ((int, str), float))
    assert direct.__origin__ is list and direct.__args__ == (int, str, float)
    assert repr(direct) == "collections.abc.Callable[[int, str], float]"
    assert repr(cls(list, (..., str))) == "list[..., str]"
    for pair in ([[], str], (int, str), ([],)):
        try:
            cls(Callable, pair)
        except TypeError:
            pass
        else:
            raise AssertionError(pair)

    class Child(Callable):
        def __call__(self):
            return 1

    child = Child[[], int]
    assert child.__origin__ is Child
    assert type(child) is cls and child().__orig_class__ == child
