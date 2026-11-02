from collections.abc import Iterable as IterableABC
from collections.abc import Reversible as ReversibleABC, Sized as SizedABC
from typing import Iterable, Reversible, Sized, get_args, get_origin


def current():
    assert get_origin(Iterable) is IterableABC
    assert Iterable.__doc__ == "Deprecated alias to collections.abc.Iterable."
    specialized = Iterable[int]
    assert specialized is Iterable[int]
    assert get_origin(specialized) is IterableABC and get_args(specialized) == (int,)
    assert repr(specialized) == "typing.Iterable[int]"

    class Items:
        def __iter__(self):
            return iter(())

    assert issubclass(Items, Iterable) and isinstance(Items(), Iterable)
    assert isinstance([], Iterable) and not isinstance(1, Iterable)
    for alias in (Iterable, specialized):
        try:
            alias()
        except TypeError as error:
            assert str(error) == "Can't instantiate abstract class Iterable without an implementation for abstract method '__iter__'"
        else:
            raise AssertionError("instantiated abstract Iterable")
    assert get_origin(Reversible[int]) is ReversibleABC
    assert isinstance([], Reversible) and not issubclass(int, Reversible)
    assert get_origin(Sized[()]) is SizedABC and get_args(Sized[()]) == ()
    try:
        Sized[int]
    except TypeError as error:
        assert str(error) == "typing.Sized is not a generic class"
    else:
        raise AssertionError("accepted a type argument for Sized")
    print("Iterable alias")
