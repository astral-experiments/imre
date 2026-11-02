from collections import namedtuple


class Start:
    def __index__(self):
        return -2


class Payload(tuple):
    def __iter__(self):
        raise AssertionError("unexpected iteration")

    def __getitem__(self, key):
        raise AssertionError("unexpected subscription")


class Broken:
    def __eq__(self, other):
        raise RuntimeError("comparison failed")


# Inherited lookup must work before any direct tuple.index access has warmed it.
assert Payload.index(Payload((4,)), 4) == 0


class Foreign:
    find = tuple.index


assert tuple.index.__objclass__ is tuple
try:
    Foreign().find
except TypeError as error:
    assert str(error) == "descriptor 'index' for 'tuple' objects doesn't apply to a 'Foreign' object"
else:
    raise AssertionError("tuple.index bound to a foreign receiver")


needle = Broken()
values = Payload((2, 4, 2, needle))
assert values.index(2, Start()) == 2
assert tuple.index(values, 4, -(10**100), 10**100) == 1
assert values.index(needle, 3) == 3
assert namedtuple("Point", "x y")(5, 7).index(7) == 1
for call, error, message in (
    (lambda: values.index(4, 2, -1), ValueError, "tuple.index(x): x not in tuple"),
    (lambda: values.index(object(), 3), RuntimeError, "comparison failed"),
    (lambda: values.index(2, start=1), TypeError, "tuple.index() takes no keyword arguments"),
    (lambda: tuple.index([], 1), TypeError, "descriptor 'index' for 'tuple' objects doesn't apply to a 'list' object"),
):
    try:
        call()
    except error as exc:
        assert str(exc) == message
    else:
        raise AssertionError("index search unexpectedly succeeded")
