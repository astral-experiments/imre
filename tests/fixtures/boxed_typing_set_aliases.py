import typing
from typing import FrozenSet, Set, TypeVar, get_args, get_origin

assert Set is typing.Set and FrozenSet is typing.FrozenSet
T = TypeVar("T")
for alias, origin in ((Set, set), (FrozenSet, frozenset)):
    assert get_origin(alias) is origin
    assert get_args(alias) == ()
    assert get_origin(alias[int]) is origin
    assert get_args(alias[int]) == (int,)
    assert alias[int] is alias[int]
    generic = alias[T]
    assert generic.__parameters__ == (T,)
    assert generic[int] == alias[int]
    assert generic[int] is generic[int]
    for args in ((), (int, str)):
        try:
            alias[args]
        except TypeError:
            pass
        else:
            raise AssertionError("accepted wrong arity")
    for constructor in (alias, generic, alias[int]):
        try:
            constructor()
        except TypeError as error:
            assert str(error) == f"Type {alias._name} cannot be instantiated; use {origin.__name__}() instead"
        else:
            raise AssertionError("constructed an instance directly")


class Mutable(Set[int]):
    pass


class Frozen(FrozenSet[int]):
    pass


mutable = Mutable([1, 2])
mutable.add(3)
assert type(mutable) is Mutable and mutable == {1, 2, 3}
frozen = Frozen([1, 2])
assert type(frozen) is Frozen and frozen == frozenset([1, 2])
