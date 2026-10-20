# ruff: noqa: B012, PLW0120, SIM107
# These completion paths intentionally test iterator cleanup and finally returns.


def record(log: list[int], value: int) -> int:
    log.append(value)
    return value


def first(values: list[int], log: list[int]) -> int:
    for value in (record(log, item) for item in values):
        try:
            if value < 0:
                continue
            return value
        finally:
            print("finally " + str(value))
    else:
        return -10


def override() -> int:
    for value in (item for item in [3, 4]):
        try:
            return value
        finally:
            return 9
    return 0


log: list[int] = []
print(first([-1, 2, 3], log))
for value in log:
    print(value)
print(first([], log))
print(override())

for value in (item for item in [1, 2, 3]):
    if value == 2:
        continue
    print(value)
else:
    print("generator exhausted")
for left, right in zip([1, 2], [3, 4], strict=True):
    print(left + right)
    break
else:
    print("unexpected zip else")
for value in reversed([1, 2]):
    print(value)
else:
    print("reverse exhausted")

try:
    for value in (10 // item for item in [2, 0, 1]):
        print(value)
    else:
        print("unexpected failure else")
except ZeroDivisionError:
    print("generator error")
finally:
    print("outer finally")

# The target survives iteration; the generator's private target does not replace it.
target = 80
value = -1
for value in (target for target in [6, 7]):
    print(value)
print(target)
print(value)

# List mutations remain visible while a generator is suspended between pulls.
values = [1, 2]
for value in (item for item in values):
    print(value)
    if value == 1:
        values.append(3)
        values[1] = 20
print(len(values))

for outer in (item for item in [1, 2]):
    for inner in (item for item in [3, 4]):
        if inner == 3:
            continue
        print(outer + inner)
        break
else:
    print("nested generators exhausted")
