from copy import deepcopy

numbers = [1]
alias = numbers
alias.extend(["text"])
alias.extend((2, "other"))
alias.extend((True,))
print(numbers, numbers is alias)

inplace = [1]
updated = inplace
updated += [True]
updated += ("other",)
print(inplace, updated is inplace)

floats = [1.0]
other = floats
other[0] = 1
print(floats, floats is other, type(floats[0]) is int)

names = {"one"}
same = names
same.add(1)
names.add(True)
print(names is same, names == {1, "one"}, {1, True})

empty = {}
print(empty)
print([{} for _ in range(0)])
print([{} for _ in range(2)])
entries = {"one": 1}
ref = entries
ref["one"] = False
entries["two"] = "two"
print(entries is ref, entries)
print({1: 2, "one": 3}, {1: 2, 3: "four"}, {1: 2, 3: True})
print({1: "first", True: "last"}, {True: "first", 1: "last"}, {1.0: 2})


class Label:
    def __repr__(self):
        return "label"


key = Label()
lookup = {key: 1}
print(lookup, lookup[key], list(lookup)[0] is key)

cycle = []
cycle.append(cycle)
copied = deepcopy(cycle)
print(cycle[0] is cycle, copied[0] is copied, copied is not cycle)
