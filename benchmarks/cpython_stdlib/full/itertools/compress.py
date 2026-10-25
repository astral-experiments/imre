"""Exercise live data and selector cursors, including the last unsuccessful pull."""

from itertools import compress, count, islice


def rhs() -> list[int]:
    print("evaluated selectors")
    return [1]


data = count(10)
chosen = compress(selectors=[False, True], data=data)
print(next(chosen))
print(next(chosen, -1))
print(next(chosen, -1))
print(next(data))

print("".join(compress("abcdef", ["", "x", "", "y", "", "z"])))
print("".join(compress("abcd", [[], [0], [], [2, 3]])))
print("".join(compress("abc", [0.0, -0.0, float("nan")])))
empty_dict: dict[int, int] = {}
print("".join(compress("abc", [{0: 0}, empty_dict, {1: 0}])))

# The same cursor is advanced twice, in data-then-selector order.
shared = islice(range(7), None)
print(repr(list(compress(shared, shared))))

selectors = count()
empty = compress("", selectors)
print(next(empty, "!"))
print(next(selectors))

live = {1: 0, 2: 0, 3: 0}
stream = count()
sel = compress(stream, live)
print(next(sel))
live[4] = 0
for _ in range(2):
    try:
        next(sel)
    except RuntimeError as err:
        print(str(err))
print(next(stream))

try:
    compress(None, rhs())
except TypeError as err:
    print(str(err))
try:
    compress(selectors=3, data=[1])
except TypeError as err:
    print(str(err))
