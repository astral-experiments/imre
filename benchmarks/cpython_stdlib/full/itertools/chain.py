"""Constructor, source acquisition, and error timing; compare stdout with CPython."""

import csv
from io import StringIO
from itertools import batched, chain, islice

items = [1, 2]
later = {3: 0}
it = chain(items, later)
alias = iter(it)
print(next(alias))
items.append(4)
later[5] = 0
print(next(it))
print(next(it))
print(next(it))
# Mutating the active iterator is observable on subsequent pulls too.
later[6] = 0
for _ in range(2):
    try:
        next(alias)
    except RuntimeError as err:
        print(str(err))

sources = ["ab", "", "c"]
nested = chain.from_iterable(sources)
print(next(nested))
sources.append("de")
print("".join(nested))
print(next(nested, "!"))
print("".join(chain.from_iterable(("x", "y"))))

# An error obtaining a new inner iterator permanently exhausts the outer one.
bad = chain([1], 42, [2])
print(next(bad))
try:
    next(bad)
except TypeError as err:
    print(str(err))
print(next(bad, -1))
bad_inner = chain.from_iterable([0, 1])
try:
    next(bad_inner)
except TypeError as err:
    print(str(err))
print(next(bad_inner, -1))

# from_iterable acquires its outer argument at construction, even without a pull.
try:
    chain.from_iterable(42)
except TypeError as err:
    print(str(err))
deferred = chain(False)
print("constructed positional chain")
try:
    next(deferred)
except TypeError as err:
    print(str(err))
print(next(deferred, -1))

outer = {"ab": 0, "z": 0}
changed = chain.from_iterable(outer)
outer["q"] = 0
try:
    next(changed)
except RuntimeError as err:
    print(str(err))
print(next(changed, "!"))

# An inner cursor is shared by identity, even when supplied more than once.
inner = islice("abcd", 4)
doubled = chain(inner, inner)
print(next(doubled))
print(next(inner))
print("".join(doubled))
empty = chain()
print(next(empty, 99))

# A starred call snapshots the outer arguments; from_iterable holds it live.
first = [1]
outer_lists = [first]
spread = chain(*outer_lists)
live = chain.from_iterable(outer_lists)
outer_lists.append([99])
first.append(2)
print(list(spread))
print(list(live))
print(list(chain([0], *outer_lists, [3])))

# The original batched test expands a list of tuples whose lengths differ.
batches = list(batched('ABCDEFG', 3))
print(''.join(chain(*batches)))

# An inner error leaves its cursor usable; an outer error ends the whole chain.
rows = csv.reader(['"bad"x', 'ok'], strict=True)
row_chain = chain(rows, [['tail']])
try:
    next(row_chain)
except csv.Error as error:
    print(str(error))
print(list(row_chain))
print(list(rows))
outer_rows = csv.reader(['"bad"x', 'ok'], strict=True)
fields = chain.from_iterable(outer_rows)
try:
    next(fields)
except csv.Error as error:
    print(str(error))
print(list(fields))
print(list(outer_rows))

# Positional chain acquires each stream on demand; from_iterable acquires its
# outer stream immediately. Direct reads and seeks use the same cursor.
text = StringIO('a\r\nb\nc', newline='')
lines = chain(['first'], text, ['last'])
print(next(lines))
print(repr(text.readline()))
print(repr(next(lines)))
text.seek(0)
print(repr(next(lines)))
text.close()
try:
    next(lines)
except ValueError as error:
    print(str(error))
closed_later = chain(['prefix'], text, ['unreachable'])
print(next(closed_later))
try:
    next(closed_later)
except ValueError as error:
    print(str(error))
print(next(closed_later, 'ended'))
try:
    chain.from_iterable(text)
except ValueError as error:
    print(str(error))
