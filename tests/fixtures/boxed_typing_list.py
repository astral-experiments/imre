import typing
from typing import Generic, List, TypeVar, get_args, get_origin


class Employee:
    pass


assert repr(List) == 'typing.List'
assert get_origin(List) is list
assert get_args(List) == ()
assert isinstance([], List) and issubclass(list, List)
assert not isinstance({}, List)
assert List[Employee] is List[Employee]
assert get_args(List[Employee])[0] is Employee
assert get_origin(List[Employee]) is list
assert type(List[int]) is typing._GenericAlias
assert repr(List[int]) == 'typing.List[int]'
assert List[int] != list[int]
assert List.append is List[int].append is list.append
assert List.__name__ == List[int].__qualname__ == 'List'
assert List[None].__args__ == (type(None),)

T = TypeVar('T')
alias = List[T]
assert alias.__parameters__ == (T,)
assert alias[int] == List[int] and alias[int] is not List[int]
assert alias[int] is alias[int]
assert List[list[T]][int] == List[list[int]]
assert get_args(List[T] | bytes) == (List[T], bytes)


class Employees(List[Employee]):
    pass


assert Employees.__orig_bases__ == (List[Employee],)
assert Employees.__bases__ == (list, Generic)
assert issubclass(Employees, list)

for constructor in (List, List[int]):
    try:
        constructor()
    except TypeError as error:
        assert str(error) == 'Type List cannot be instantiated; use list() instead'
    else:
        raise AssertionError('List alias constructed an instance')
try:
    List[int, str]
except TypeError as error:
    assert str(error) == 'Too many arguments for typing.List; actual 2, expected 1'
else:
    raise AssertionError('List accepted two arguments')
