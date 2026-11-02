def concatenate(left, right):
    return left + right


def allocation():
    first, last = object(), object()
    left, right = (first,) * 1025, (last,) * 1028
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        concatenate(left, right)
    except MemoryError as error:
        print("caught", error.args)
    result = concatenate(left, right)
    print(len(left), len(right), len(result), result[0] is first, result[-1] is last)


class Payload(tuple):
    def __iter__(self):
        raise AssertionError("iterated while concatenating")


class Reflected(tuple):
    def __radd__(self, other):
        return "reflected"


class Declines(tuple):
    def __add__(self, other):
        return NotImplemented


class PicksLeft(tuple):
    def __add__(self, other):
        return "forward"


class PicksRight(PicksLeft):
    def __radd__(self, other):
        return "subtype"


source = tuple([1, 2])
payload = Payload(source)
for left, right, expected in (
    (payload, (3,), (1, 2, 3)),
    ((3,), payload, (3, 1, 2)),
    (payload, payload, (1, 2, 1, 2)),
    ((), payload, source),
    (payload, (), source),
):
    result = concatenate(left, right)
    assert type(result) is tuple and result == expected and result is not source

assert concatenate(Payload(), source) is source
assert concatenate(source, Payload()) is source
assert concatenate(PicksLeft(), PicksRight()) == "subtype"
assert concatenate(PicksLeft(), Reflected()) == "forward"
assert concatenate(Declines(), Reflected()) == "reflected"
assert concatenate(Reflected((1,)), PicksRight((2,))) == (1, 2)
for left, right, message in (
    (Declines(), (), "unsupported operand type(s) for +: 'Declines' and 'tuple'"),
    (payload, [], 'can only concatenate tuple (not "list") to tuple'),
):
    try:
        concatenate(left, right)
    except TypeError as exc:
        assert str(exc) == message
    else:
        raise AssertionError("invalid tuple concatenation succeeded")
