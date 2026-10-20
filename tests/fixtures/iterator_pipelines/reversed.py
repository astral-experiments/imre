# ruff: noqa: PLW0120
# Exhaustion suites test the reverse iterator's termination after mutation.


def grow(values: list[int]) -> int:
    values.append(8)
    return 10


def shrink(values: list[int]) -> int:
    values.pop()
    values.pop()
    return 10


values = [1, 2, 3]
for value in reversed(values):
    print(value)
    if value == 3:
        values[1] = 20
        values.append(4)
print(len(values))

values = [1, 2, 3]
for value in reversed(values):
    print(value)
    values.pop()
    if value == 3:
        values.pop()
else:
    print("shrunken reverse exhausted")

# The initial index belongs to reversed(), before evaluating sum's start.
values = [1, 2]
print(sum(reversed(values), grow(values)))
print(len(values))
values = [1, 2, 3]
print(sum(reversed(values), shrink(values)))
print(len(values))

values = [1, 2, 3]
old = values
for value in reversed(values):
    values = [99]
    print(value)
print(len(old))
print(values[0])

for text in ["", "a", "é好🙂", "e\u0301", "\x00🙂é"]:
    for character in reversed(text):
        print(character)
    print("end string")

for start, stop, step in [(0, 0, 1), (0, 7, 2), (7, -2, -3), (2, 7, -1)]:
    for value in reversed(range(start, stop, step)):
        print(value)
    print("end range")
huge = 1000000000000000000000000000000
for value in reversed(range(huge, huge + 7, 2)):
    print(value)
for value in reversed(range(-huge, -huge - 7, -2)):
    print(value)
