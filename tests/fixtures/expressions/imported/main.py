"""Runtime argv initialization must see uses nested inside conditional arms."""

from helper import arguments, choose, count

print(count(True))
print(count(False))
values = arguments(True)
values[0] = "rewritten"
print(arguments(True)[0])
print(len(arguments(False)))
print(choose(False))
