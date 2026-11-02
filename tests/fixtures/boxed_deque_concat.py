from collections import deque


def current():
    marker = object()
    left = deque([marker], maxlen=2)
    right = deque([2], maxlen=1)
    result = left + right
    assert result.maxlen == 2 and result[0] is marker and result[1] == 2
    assert result is not left and result is not right and len(left) == len(right) == 1
    assert (right + left).maxlen == 1 and (right + left)[0] is marker
    assert list(left + left) == [marker, marker]
    assert list(deque(maxlen=0) + left) == []
    assert deque.__dict__["__add__"] is deque.__add__

    calls = []

    class Child(deque):
        def __init__(self, *args):
            calls.append(args)
            super().__init__(*args)

        def __iter__(self):
            calls.append("iter")
            return super().__iter__()

        def copy(self):
            raise AssertionError("dispatched to copy")

        __copy__ = copy

        def extend(self, iterable):
            raise AssertionError("dispatched to extend")

    for args in (([marker],), ([marker], 2)):
        child = Child(*args)
        calls.clear()
        copy = child + child
        assert type(copy) is Child and copy is not child and copy[0] is copy[1] is marker
        init_args, *iterations = calls
        assert init_args[0] is child and init_args[1:] == args[1:]
        assert iterations == ["iter", "iter"] and len(child) == 1

    class Allocator(deque):
        def __new__(cls, source=None):
            if source is None:
                return deque.__new__(cls)
            return source[0]

    original = Allocator()
    alias = deque([7])
    original.append(alias)
    # The custom copy constructor returns the right operand; extension must snapshot itself.
    assert original + alias is alias and list(alias) == [7, 7]
    original[0] = []
    try:
        original + left
    except TypeError as exc:
        assert str(exc) == "Allocator() must return a deque, not list"
    else:
        raise AssertionError("accepted a non-deque copy result")

    class Mask:
        @property
        def __class__(self):
            return deque

        def __iter__(self):
            return iter([3])

    assert list(left + Mask()) == [marker, 3]

    class Reflected:
        def __radd__(self, other):
            return other

    assert left + Reflected() is left
    try:
        deque.__add__(original, Reflected())
    except TypeError as exc:
        assert str(exc) == 'can only concatenate deque (not "Reflected") to deque'
    else:
        raise AssertionError("descriptor accepted a non-deque operand")

    class Override(deque):
        def __add__(self, other):
            return NotImplemented

    declined = Override([1])
    assert declined + Reflected() is declined
    try:
        declined + left
    except TypeError as exc:
        assert "unsupported operand type(s) for +" in str(exc)
    else:
        raise AssertionError("ignored a declining __add__")
    assert list(deque.__add__(declined, left)) == [1, marker]

    print("deque concat passed")
