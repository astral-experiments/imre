from functools import cmp_to_key


def current():
    import functools
    import inspect
    import sys
    from types import ModuleType

    assert functools.cmp_to_key is cmp_to_key
    assert str(inspect.signature(cmp_to_key)) == "(mycmp)"
    owner = cmp_to_key.__self__
    assert owner is sys.modules[cmp_to_key.__module__]
    try:
        sys.modules[cmp_to_key.__module__] = ModuleType("replacement")
        assert cmp_to_key.__self__ is owner
    finally:
        sys.modules[cmp_to_key.__module__] = owner

    descending = cmp_to_key(mycmp=lambda x, y: y - x)
    assert descending.obj is None
    assert sorted([2, 3, 1], key=descending) == [3, 2, 1]
    wrapped = descending(obj=2)
    assert wrapped(7).obj == 7
    wrapped.obj = 9
    assert wrapped < descending(3)
    slot = type(wrapped).__lt__
    assert slot.__get__(None, type(wrapped)) is slot
    assert slot.__get__(wrapped)(descending(3))
    try:
        slot.__get__(3)
    except TypeError as exc:
        assert "doesn't apply to a 'int' object" in str(exc)
    else:
        raise AssertionError("bound a KeyWrapper descriptor to an int")
    del wrapped.obj
    assert wrapped.obj is None
    try:
        wrapped < descending(3)
    except AttributeError as exc:
        assert str(exc) == "object"
    else:
        raise AssertionError("compared a key without an object")

    marker = object()

    class Result:
        def __le__(self, zero):
            assert zero == 0
            return marker

    def compare(left, right):
        assert (left, right) == (2, 5)
        return Result()

    assert (cmp_to_key(compare)(2) <= descending(5)) is marker
    try:
        descending(3) == 3
    except TypeError as exc:
        assert str(exc) == "other argument must be K instance"
    else:
        raise AssertionError("compared a key to a non-key")
    print("boxed cmp_to_key passed")
