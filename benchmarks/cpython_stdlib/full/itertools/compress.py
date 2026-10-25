"""Exercise live data and selector cursors, including the last unsuccessful pull."""

import csv
from io import StringIO
from itertools import batched, compress, count, islice, repeat


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

# Stream objects are true even when closed. Testing their truth must not read
# from them, while using the same stream for both inputs pulls two lines.
shared_text = StringIO('A\n1\nB\n2\n')
print(''.join(compress('xy', repeat(shared_text, 2))))
print(shared_text.tell())
paired = compress(shared_text, shared_text)
print(repr(next(paired)))
print(repr(shared_text.readline()))
print(next(paired, 'ended'))
shared_text.close()
print(''.join(compress('z', repeat(shared_text, 1))))
try:
    compress('never read', shared_text)
except ValueError as error:
    print(str(error))

# Selecting an iterator calls its truth test without consuming it.
empty_cursor = islice('hi', 0)
print(''.join(compress('xy', [empty_cursor, empty_cursor])))
labels: list[str] = []
print(''.join(compress('abc', [labels, [''], labels])))
print(''.join(compress('xy', batched('ab', 1))))

# A data error precedes that pair's selector pull. A selector error loses its
# associated data item; on the next call the following pair is independent.
bad_data = csv.reader(['"bad"x', 'good'], strict=True)
positions = count(1)
rows = compress(bad_data, positions)
try:
    next(rows)
except csv.Error as err:
    print(str(err))
print(next(positions))
print(next(rows))
print(next(positions))

bad_selectors = csv.reader(['"bad"x', '', 'yes'], strict=True)
numbers = count(10)
selected = compress(numbers, bad_selectors)
try:
    next(selected)
except csv.Error as err:
    print(str(err))
print(next(selected))
print(next(numbers))
try:
    compress(selectors=3, data=[1])
except TypeError as err:
    print(str(err))
