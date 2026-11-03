from operator import getitem
from typing import Callable, Tuple, TypeVar, get_args, get_origin


def current():
    assert get_origin(Tuple) is tuple
    empty = Tuple[()]
    fixed = Tuple[int, str]
    repeated = Tuple[int, ...]
    assert empty.__args__ == ()
    assert repr(empty) == "typing.Tuple[()]"
    assert fixed is Tuple[int, str]
    assert fixed.__origin__ is tuple and get_args(fixed) == (int, str)
    assert get_args(repeated) == (int, ...) and repeated != fixed
    assert get_args(Tuple[None]) == (type(None),)
    for alias, origin, args in (
        (empty, Tuple, ()),
        (Tuple[int], Tuple, int),
        (fixed, Tuple, (int, str)),
        (Callable[[], int], Callable, ([], int)),
        (Callable[[int, str], bytes], Callable, ([int, str], bytes)),
        (Callable[..., bytes], Callable, (..., bytes)),
    ):
        reconstruct, operands = alias.__reduce__()
        assert reconstruct is getitem and operands == (origin, args)
        assert reconstruct(*operands) == alias
    t = TypeVar("T")
    assert Tuple[t, t][bytes] == Tuple[bytes, bytes]
    try:
        Tuple[(int,), ...]
    except TypeError as error:
        assert str(error).startswith("Tuple[t, ...]: t must be a type.")
    else:
        raise AssertionError("Tuple accepted a tuple value as a type")
    calls = []

    class Pack:
        __typing_is_unpacked_typevartuple__ = True
        result = (str, bytes)

        @property
        def __args__(self):
            raise AssertionError("looked inside a substitution hook")

        def __typing_prepare_subst__(self, alias, items):
            calls.append(("prepare", alias is schema, items))
            return None

        def __typing_subst__(self, item):
            calls.append(("subst", item))
            return self.result

    pack = Pack()
    schema = tuple[pack, bool]
    assert schema[int].__args__ == (str, bytes, bool)
    pack.result = 42
    try:
        schema[int]
    except TypeError as error:
        assert str(error).endswith("Pack objects to return a tuple, not int")
    else:
        raise AssertionError("accepted a scalar from an unpacked substitution hook")
    assert calls == [("prepare", True, (int,)), ("subst", None)] * 2

    class Key(str):
        def __typing_subst__(self, item):
            return item

    key = Key("key")

    class Member:
        def __init__(self, parameter):
            self.__parameters__ = (parameter,)

        def __getitem__(self, args):
            return args

    schema = tuple[Member(key), key]
    assert schema[((int, str),)].__args__ == ((int, str), (int, str))
    print("tuple aliases")
