"""Constructor, source acquisition, and error timing; compare stdout with CPython."""

from itertools import chain, islice

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
