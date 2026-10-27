from operator import getitem
from types import UnionType
from typing import Union, get_args, get_origin


class Left:
    pass


class Right:
    pass


class Other:
    def __ror__(self, value):
        return value + 10


class EqualSeventeen:
    def __hash__(self):
        return hash(17)

    def __eq__(self, other):
        return other == 17


class RaisesEquality(EqualSeventeen):
    __hash__ = EqualSeventeen.__hash__

    def __eq__(self, other):
        raise ValueError("union member equality")


def current():
    left = list[EqualSeventeen()] | str
    right = str | list[17]
    assert left == right and {left: 42}[right] == 42
    assert frozenset(((left,),)) == frozenset(((right,),))
    try:
        frozenset(((list[RaisesEquality()] | str,),)) == frozenset(((right,),))
    except ValueError as error:
        assert str(error) == "union member equality"
    else:
        raise AssertionError("nested union comparison suppressed the member exception")
    union = int | str
    reverse = str | int
    print(type(union) is UnionType is Union, get_origin(union) is Union)
    print(union.__args__ == (int, str), get_args(union) is union.__args__)
    print(getitem(Union, union.__args__) == union, (int | int) is int)
    print(hash(union) == hash(reverse) == hash(frozenset(union.__args__)), {union: 42}[reverse])
    nested = Left | (Right | Left)
    print(nested.__args__ == (Left, Right), nested == Right | Left)
    print(isinstance(Left(), nested), issubclass(str, union), isinstance(None, int | None))
    print(2 | 4, 3 | Other())
