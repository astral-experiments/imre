from operator import getitem
from types import UnionType
from typing import TypeVar, Union, get_args, get_origin


T = TypeVar("T")


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
    import gc
    from typing import Dict, ForwardRef, GenericAlias

    assert GenericAlias is type(list[int])

    class Symlink(str):
        pass

    recursive = Dict[str, Union[str, bytes, Symlink, 'FilesSpec']]
    assert recursive.__args__[1].__args__ == (str, bytes, Symlink, ForwardRef('FilesSpec'))
    assert type(Union['A', 'A']) is ForwardRef
    assert (int | str | 'A').__args__ == (int, str, ForwardRef('A'))
    assert ('A' | (int | str)).__args__ == (ForwardRef('A'), int, str)
    import copy
    import pickle
    from types import GenericAlias

    alias = list[T]
    assert alias.__reduce__() == (GenericAlias, (list, (T,)))
    iterator = iter(alias)
    for owner in (GenericAlias, type(iterator)):
        assert owner.__reduce__.__doc__ is None
        assert owner.__reduce__.__text_signature__ == "($self, /)"
    assert iterator.__reduce__() == (iter, (alias,))
    starred = next(iterator)
    assert iterator.__reduce__() == (iter, ((),))
    factory, (fresh,) = starred.__reduce__()
    assert factory is next and fresh.__reduce__() == (iter, (alias,))
    ns = {"x": starred, "__builtins__": {"next": "replacement"}}
    exec("result = x.__reduce__()", ns)
    assert ns["result"][0] == "replacement"
    iterator = iter(alias)

    class Builtins(dict):
        def __missing__(self, name):
            assert name == "iter"
            next(iterator)
            return "replacement"

    ns = {"x": iterator, "__builtins__": Builtins()}
    exec("result = x.__reduce__()", ns)
    assert ns["result"] == ("replacement", ((),))
    for original in (starred, alias | int):
        assert copy.copy(original) == copy.deepcopy(original) == original
        for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
            result = pickle.loads(pickle.dumps(original, protocol))
            assert result == original and result.__parameters__ == (T,)

    variable = TypeVar('Variable')
    assert (variable | int)[int] is int
    assert (list[variable] | int)[str].__args__ == (list[str], int)
    try:
        (variable | int)[int, str]
    except TypeError:
        pass
    else:
        raise AssertionError('union accepted too many type arguments')
    assert repr(Union[current, int]) == f'{__name__}.{current.__qualname__} | int'
    assert (variable | 'A').__args__ == (variable, ForwardRef('A'))
    for operand in (int, list[int]):
        try:
            operand | 'A'
        except TypeError:
            pass
        else:
            raise AssertionError('an ordinary type cannot be combined with a string')
    try:
        Union['(']
    except SyntaxError as exc:
        assert str(exc) == "Forward reference must be an expression -- got '('"
    else:
        raise AssertionError('expected invalid forward reference')

    class Fault(str):
        def lstrip(self):
            raise RuntimeError("from original lstrip")

    try:
        Union[Fault("\ud800")]
    except RuntimeError as exc:
        assert str(exc) == "from original lstrip"
    else:
        raise AssertionError("forward reference skipped str subclass code")

    class Collecting:
        def __hash__(self):
            gc.collect()
            return 42

    member = Collecting()
    args = Union[member, 'A', 'B', 'A'].__args__
    assert args == (member, ForwardRef('A'), ForwardRef('B'))

    class Member:
        def __ror__(self, left):
            return "reflected"

    member = Member()
    for left_operand in (int | str, TypeVar("T")):
        assert (left_operand | member).__args__[-1] is member
    assert list[int] | member == "reflected"

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
    for name in ('__args__', '__parameters__', '__origin__'):
        assert name in dir(union)
        descriptor = vars(Union)[name]
        assert descriptor.__objclass__ is Union
        assert descriptor.__get__(union, Union) is getattr(union, name)
    for base, message in ((Union, "type 'typing.Union' is not an acceptable base type"),
                          (union, 'Cannot subclass int | str')):
        try:
            class Invalid(base):
                pass
        except TypeError as exc:
            assert str(exc) == message
        else:
            raise AssertionError('union accepted as a base')

    class UnhashableMeta(type):
        __hash__ = None

    class Unhashable(metaclass=UnhashableMeta):
        pass

    for value in (Unhashable, Union[int, Unhashable]):
        try:
            hash(value)
        except TypeError as exc:
            assert str(exc) == "unhashable type: 'UnhashableMeta'"
        else:
            raise AssertionError('expected a TypeError from an unhashable class')
    print(type(union) is UnionType is Union, get_origin(union) is Union)
    print(union.__args__ == (int, str), get_args(union) is union.__args__)
    print(getitem(Union, union.__args__) == union, (int | int) is int)
    print(hash(union) == hash(reverse) == hash(frozenset(union.__args__)), {union: 42}[reverse])
    nested = Left | (Right | Left)
    print(nested.__args__ == (Left, Right), nested == Right | Left)
    print(isinstance(Left(), nested), issubclass(str, union), isinstance(None, int | None))
    print(2 | 4, 3 | Other())
