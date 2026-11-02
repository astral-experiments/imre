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
    print("tuple aliases")
