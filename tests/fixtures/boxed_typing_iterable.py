from collections.abc import Iterable as IterableABC
from typing import Iterable, get_args, get_origin


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
    print("Iterable alias")
